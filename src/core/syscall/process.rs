//! プロセス管理関連のシステムコール

use super::types::{EFAULT, EINVAL, ENOMEM, ENOSYS, SUCCESS};

/// ユーザー空間の上限アドレス (x86-64 canonical hole 下側)
const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;
/// Linux互換: 子プロセスが存在しない
const ECHILD: u64 = (-10i64) as u64;
/// PIT割り込み周期 (10ms)
const TICK_MS: u64 = 10;
use crate::task::{current_thread_id, exit_current_task};

#[inline]
fn page_align_up(addr: u64) -> Option<u64> {
    addr.checked_add(4095).map(|v| v & !4095)
}

#[inline]
fn is_user_range(addr: u64, len: u64) -> bool {
    if len == 0 {
        return addr <= USER_SPACE_END;
    }
    let end = match addr.checked_add(len.saturating_sub(1)) {
        Some(e) => e,
        None => return false,
    };
    addr <= USER_SPACE_END && end <= USER_SPACE_END
}

/// Exitシステムコール
///
/// プロセスを終了する
///
/// # 引数
/// - `exit_code`: 終了コード
///
/// # 戻り値
/// このシステムコールは戻らない（プロセスが終了する）
pub fn exit(exit_code: u64) -> ! {
    crate::sprintln!("Process exiting with code: {}", exit_code);

    // スケジューラから現在のタスクを削除して終了
    exit_current_task(exit_code)
}

/// GetPidシステムコール
///
/// 現在のプロセスIDを取得する
///
/// # 戻り値
/// プロセスID
pub fn getpid() -> u64 {
    if let Some(tid) = current_thread_id() {
        crate::task::with_thread(tid, |thread| thread.process_id().as_u64()).unwrap_or(0)
    } else {
        0
    }
}

/// GetTidシステムコール
///
/// 現在のスレッドIDを取得する
///
/// # 戻り値
/// スレッドID
pub fn gettid() -> u64 {
    if let Some(tid) = current_thread_id() {
        tid.as_u64()
    } else {
        0
    }
}

/// Brkシステムコール
///
/// メモリのヒープ領域サイズを変更する
pub fn brk(addr: u64) -> u64 {
    // 現在のプロセスIDを取得
    let current_tid = match current_thread_id() {
        Some(tid) => tid,
        None => return ENOSYS,
    };

    // プロセスIDを取得
    let pid = match crate::task::with_thread(current_tid, |t| t.process_id()) {
        Some(pid) => pid,
        None => return ENOSYS,
    };

    let result = crate::task::with_process_mut(pid, |process| {
        if process.heap_start() == 0 {
            let default_heap_base = 0x4000_0000;
            process.set_heap_start(default_heap_base);
            process.set_heap_end(default_heap_base);
        }
        // addr == 0 なら現在の位置を返す
        if addr == 0 {
            return Ok(process.heap_end());
        }

        if addr < process.heap_start() {
            return Err(EINVAL);
        }

        let current_brk = process.heap_end();

        // ユーザー空間の上限アドレスを超えるbrkを拒否
        if !is_user_range(addr, 1) {
            return Err(EINVAL);
        }

        // 縮小または変化なし
        if addr <= current_brk {
            process.set_heap_end(addr);
            return Ok(addr);
        }

        // プロセス固有のページテーブルアドレスを取得
        let pt_phys = match process.page_table() {
            Some(p) => p,
            None => return Err(ENOSYS),
        };

        // 拡大時にページをプロセスのページテーブルにマップ（書き込み可能、実行不可）
        // 現在の brk がページ境界でない場合に、既に存在するページを含めてマップするために
        // floor(current_brk) を使用する。
        let start_page = current_brk & !4095;
        let end_page = match page_align_up(addr) {
            Some(v) if is_user_range(v.saturating_sub(1), 1) => v,
            _ => return Err(EINVAL),
        };

        if end_page > start_page {
            let size = end_page - start_page;
            if let Err(_) = crate::mem::paging::map_and_copy_segment_to(
                pt_phys,
                start_page,
                0,
                size,
                &[],
                true,
                false,
            ) {
                return Err(ENOSYS);
            }
        }

        process.set_heap_end(addr);
        Ok(addr)
    });

    match result {
        Some(Ok(addr)) => addr,
        Some(Err(err)) => err,
        None => ENOSYS,
    }
}

/// Forkシステムコール
///
/// プロセスを複製する
pub fn fork() -> u64 {
    // CRIT-03/HIGH-01 対応: ページテーブル共有実装とグローバル一時保存値依存は
    // プロセス分離/整合性を破るため、安全な複製実装が入るまで fail-closed とする。
    crate::warn!(
        "fork is temporarily disabled until isolated address-space cloning is implemented"
    );
    ENOSYS
}

/// Sleepシステムコール
///
/// 指定されたミリ秒数の間スリープする
///
/// # 引数
/// - `milliseconds`: スリープ時間（ミリ秒）
///
/// # 戻り値
/// 成功時はSUCCESS
pub fn sleep(milliseconds: u64) -> u64 {
    if milliseconds == 0 {
        return SUCCESS;
    }
    let wait_ticks = milliseconds
        .checked_add(TICK_MS - 1)
        .map(|v| v / TICK_MS)
        .unwrap_or(u64::MAX);
    let target = crate::syscall::time::get_ticks().saturating_add(wait_ticks);
    crate::syscall::time::sleep_until(target);
    SUCCESS
}

/// Waitシステムコール (wait4)
///
/// # 引数
/// - `pid`: 待機するプロセスID (-1 = 任意の子プロセス)
/// - `status_ptr`: 終了ステータスを書き込むポインタ (0 = 無視)
/// - `options`: WNOHANG(0x1) = ノンブロッキング
pub fn wait(_pid: u64, status_ptr: u64, options: u64) -> u64 {
    const WNOHANG: u64 = 0x1;
    let pid = _pid as i64;
    if options & !WNOHANG != 0 {
        return EINVAL;
    }
    if pid < -1 || pid == 0 {
        return EINVAL;
    }

    if status_ptr != 0 && !super::validate_user_ptr(status_ptr, 4) {
        return EFAULT;
    }

    // 呼び出し元プロセス
    let current_pid = match current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
    {
        Some(pid) => pid,
        None => return ECHILD,
    };

    // 対象子プロセスを列挙
    let mut target_children = [0u64; 64];
    let mut target_count = 0usize;
    crate::task::for_each_process(|proc| {
        if proc.parent_id() != Some(current_pid) {
            return;
        }
        let child_pid = proc.id().as_u64();
        if pid == -1 || pid as u64 == child_pid {
            if target_count < target_children.len() {
                target_children[target_count] = child_pid;
                target_count += 1;
            }
        }
    });
    if target_count == 0 {
        return ECHILD;
    }

    if options & WNOHANG != 0 {
        // ノンブロッキング: まだ回収可能な子がなければ0
        return 0;
    }

    // 簡易ブロッキング: 子スレッドが消えるまで待機
    loop {
        for idx in 0..target_count {
            let child_pid = target_children[idx];
            let mut child_has_live_thread = false;
            crate::task::for_each_thread(|thread| {
                if thread.process_id().as_u64() == child_pid
                    && thread.state() != crate::task::ThreadState::Terminated
                {
                    child_has_live_thread = true;
                }
            });
            if !child_has_live_thread {
                if status_ptr != 0 {
                    unsafe {
                        *(status_ptr as *mut i32) = 0;
                    }
                }
                return child_pid;
            }
        }
        crate::task::yield_now();
    }
}

/// Mmapシステムコール
///
/// 匿名メモリマッピングを作成する (MAP_ANONYMOUS | MAP_PRIVATE のみサポート)
///
/// # 引数
/// - `addr`: ヒント仮想アドレス (0で任意)
/// - `length`: マップするサイズ
/// - `prot`: 保護フラグ (PROT_READ|PROT_WRITE = 3)
/// - `flags`: マップフラグ (MAP_ANONYMOUS=0x20, MAP_PRIVATE=0x2)
/// - `_fd`: ファイルディスクリプタ (-1 = 匿名)
///
/// # 戻り値
/// マップされた仮想アドレス、またはエラーコード
pub fn mmap(addr: u64, length: u64, _prot: u64, flags: u64, _fd: u64) -> u64 {
    use super::types::{EINVAL, ENOMEM};

    if length == 0 {
        return EINVAL;
    }

    // MAP_ANONYMOUS (0x20) のみサポート
    const MAP_ANONYMOUS: u64 = 0x20;
    if flags & MAP_ANONYMOUS == 0 {
        return ENOSYS;
    }

    let current_tid = match crate::task::current_thread_id() {
        Some(tid) => tid,
        None => return ENOMEM,
    };
    let pid = match crate::task::with_thread(current_tid, |t| t.process_id()) {
        Some(pid) => pid,
        None => return ENOMEM,
    };

    // ページ境界に切り上げ（オーバーフロー安全）
    let size = match page_align_up(length) {
        Some(v) if v > 0 => v,
        _ => return EINVAL,
    };

    let result = crate::task::with_process_mut(pid, |process| {
        // mmap用のヒープ領域を現在のbrk以降に割り当てる
        // (簡易実装: brkと同じ領域を使う)
        if process.heap_start() == 0 {
            let default_heap_base = 0x5000_0000u64;
            process.set_heap_start(default_heap_base);
            process.set_heap_end(default_heap_base);
        }

        // ユーザー空間の上限アドレスを超えるaddrを拒否
        if addr != 0 && addr > USER_SPACE_END {
            return Err(EINVAL);
        }

        let map_start = if addr != 0 {
            match page_align_up(addr) {
                Some(v) => v,
                None => return Err(EINVAL),
            }
        } else {
            // heap_endを mmap_base として使う（簡易実装）
            // 実際は別のアドレス空間管理が必要
            let base = process.heap_end();
            match page_align_up(base) {
                Some(v) => v,
                None => return Err(EINVAL),
            }
        };

        if !is_user_range(map_start, size) {
            return Err(EINVAL);
        }

        let pt_phys = match process.page_table() {
            Some(p) => p,
            None => return Err(ENOMEM),
        };

        if let Err(_) = crate::mem::paging::map_and_copy_segment_to(
            pt_phys,
            map_start,
            0,
            size,
            &[],
            true,
            false,
        ) {
            return Err(ENOMEM);
        }

        // heap_end を更新してアドレス空間が重ならないようにする
        if addr == 0 {
            let new_heap_end = match map_start.checked_add(size) {
                Some(v) => v,
                None => return Err(EINVAL),
            };
            process.set_heap_end(new_heap_end);
        }

        Ok(map_start)
    });

    match result {
        Some(Ok(va)) => va,
        Some(Err(e)) => e,
        None => ENOMEM,
    }
}

/// Munmapシステムコール
pub fn munmap(addr: u64, length: u64) -> u64 {
    if addr == 0 || length == 0 {
        return EINVAL;
    }
    let unmap_start = addr & !4095;
    let unmap_end = match addr.checked_add(length).and_then(page_align_up) {
        Some(v) => v,
        None => return EINVAL,
    };
    let unmap_len = match unmap_end.checked_sub(unmap_start) {
        Some(v) if v > 0 => v,
        _ => return EINVAL,
    };
    if !is_user_range(unmap_start, unmap_len) {
        return EINVAL;
    }

    let tid = match current_thread_id() {
        Some(t) => t,
        None => return ENOSYS,
    };
    let pid = match crate::task::with_thread(tid, |t| t.process_id()) {
        Some(p) => p,
        None => return ENOSYS,
    };
    let pt_phys = match crate::task::with_process(pid, |p| p.page_table()).flatten() {
        Some(p) => p,
        None => return ENOSYS,
    };

    match crate::mem::paging::unmap_range_in_table(pt_phys, unmap_start, unmap_len) {
        Ok(()) => SUCCESS,
        Err(_) => EINVAL,
    }
}

/// Futexシステムコール (最小実装)
///
/// FUTEX_WAIT と FUTEX_WAKE のみサポート
pub fn futex(uaddr: u64, op: u32, val: u64, _timeout: u64) -> u64 {
    use super::types::EAGAIN;
    const FUTEX_WAIT: u32 = 0;
    const FUTEX_WAKE: u32 = 1;
    const FUTEX_PRIVATE_FLAG: u32 = 128;

    let op_base = op & !FUTEX_PRIVATE_FLAG;

    match op_base {
        FUTEX_WAIT => {
            if uaddr == 0 {
                return EFAULT;
            }
            // ユーザー空間アドレスの有効性を検証する
            if !super::validate_user_ptr(uaddr, 4) {
                return EFAULT;
            }
            let current_val = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
            if current_val != val as u32 {
                return EAGAIN;
            }
            // 簡易実装: yield して再試行させる
            crate::task::yield_now();
            SUCCESS
        }
        FUTEX_WAKE => {
            // Wake は何もしなくても yield ベースで動く
            SUCCESS
        }
        _ => ENOSYS,
    }
}

/// arch_prctlシステムコール
///
/// TLS 用の FS ベースレジスタを設定する
pub fn arch_prctl(code: u64, addr: u64) -> u64 {
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;

    match code {
        ARCH_SET_FS => {
            // FS ベースレジスタを設定 (WRFSBASE または IA32_FS_BASE MSR)
            unsafe {
                crate::cpu::write_fs_base(addr);
            }
            // 現在のスレッドに FS base を記録 (コンテキストスイッチ時に復元するため)
            if let Some(tid) = crate::task::current_thread_id() {
                crate::task::with_thread_mut(tid, |t| t.set_fs_base(addr));
            }
            SUCCESS
        }
        ARCH_GET_FS => {
            let val = unsafe { crate::cpu::read_fs_base() };
            // addrが指すメモリに書き込む
            if addr == 0 {
                return EFAULT;
            }
            // ユーザー空間アドレスの有効性を検証する
            if !super::validate_user_ptr(addr, 8) {
                return EFAULT;
            }
            unsafe { core::ptr::write_unaligned(addr as *mut u64, val) };
            SUCCESS
        }
        _ => EINVAL,
    }
}

/// FindProcessByNameシステムコール
///
/// プロセス名からPIDを検索する
///
/// # 引数
/// - `name_ptr`: プロセス名のポインタ
/// - `len`: プロセス名の長さ
///
/// # 戻り値
/// 見つかった場合はPID、見つからない場合は0
pub fn find_process_by_name(name_ptr: u64, len: u64) -> u64 {
    use crate::task;
    use core::str;

    if name_ptr == 0 || len == 0 || len > 64 {
        return 0;
    }

    // ユーザー空間アドレスの有効性を検証する
    if !super::validate_user_ptr(name_ptr, len) {
        return 0;
    }

    let name_slice = unsafe { core::slice::from_raw_parts(name_ptr as *const u8, len as usize) };
    let name = match str::from_utf8(name_slice) {
        Ok(s) => s,
        Err(_) => return 0,
    };

    // プロセスリストを検索
    // TODO: 直接タスク管理モジュールにアクセスするのはリスキーなのでロックをかける
    // taskモジュールに検索関数を追加するのが望ましい
    task::find_process_id_by_name(name)
        .map(|pid| pid.as_u64())
        .unwrap_or(0)
}
