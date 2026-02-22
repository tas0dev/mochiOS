//! プロセス管理関連のシステムコール

use crate::task::{exit_current_task, current_thread_id};
use super::types::{SUCCESS, ENOSYS, EINVAL, EFAULT, ENOMEM};

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
        crate::task::with_thread(tid, |thread| {
            thread.process_id().as_u64()
        }).unwrap_or(0)
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
        // addr == 0 なら現在の位置を返す
        if addr == 0 {
             if process.heap_start() == 0 {
                 // ヒープ領域初期化（暫定）
                 let default_heap_base = 0x4000_0000;
                 process.set_heap_start(default_heap_base);
                 process.set_heap_end(default_heap_base);
             }
             return Ok(process.heap_end());
        }

        let current_brk = process.heap_end();

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
        let start_page = (current_brk + 4095) & !4095;
        let end_page = (addr + 4095) & !4095;

        if end_page > start_page {
            let size = end_page - start_page;
            if let Err(_) = crate::mem::paging::map_and_copy_segment_to(
                pt_phys,
                start_page,
                0,
                size,
                &[],
                true,
                false
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
    // TODO: 正確なタイマーベースのスリープを実装
    // 現在は単純にyieldするだけ（タイマー割り込みがあるので時間は経過する）
    // 最大でも数回yieldするだけにする
    let yield_count = (milliseconds / 10).max(1).min(100);

    for _ in 0..yield_count {
        crate::task::yield_now();
    }

    SUCCESS
}

/// Waitシステムコール
pub fn wait(_pid: u64, _status_ptr: u64) -> u64 {
    ENOSYS
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

    // ページ境界に切り上げ
    let size = (length + 4095) & !4095;

    let result = crate::task::with_process_mut(pid, |process| {
        // mmap用のヒープ領域を現在のbrk以降に割り当てる
        // (簡易実装: brkと同じ領域を使う)
        if process.heap_start() == 0 {
            let default_heap_base = 0x5000_0000u64;
            process.set_heap_start(default_heap_base);
            process.set_heap_end(default_heap_base);
        }

        let map_start = if addr != 0 {
            (addr + 4095) & !4095
        } else {
            // heap_endを mmap_base として使う（簡易実装）
            // 実際は別のアドレス空間管理が必要
            let base = process.heap_end();
            (base + 4095) & !4095
        };

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
            process.set_heap_end(map_start + size);
        }

        Ok(map_start)
    });

    match result {
        Some(Ok(va)) => va,
        Some(Err(e)) => e,
        None => ENOMEM,
    }
}

/// Munmapシステムコール (スタブ)
pub fn munmap(_addr: u64, _length: u64) -> u64 {
    // TODO: ページテーブルからマッピングを削除する
    SUCCESS
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
            unsafe { crate::cpu::write_fs_base(addr); }
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
            unsafe { core::ptr::write(addr as *mut u64, val) };
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
    
    // ユーザー空間から名前をコピー（安全のため制限付き）
    // 本来はユーザーメモリチェックが必要
    let name_slice = unsafe { core::slice::from_raw_parts(name_ptr as *const u8, len as usize) };
    let name = match str::from_utf8(name_slice) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    
    // プロセスリストを検索
    // TODO: 直接タスク管理モジュールにアクセスするのはリスキーなのでロックをかける
    // taskモジュールに検索関数を追加するのが望ましい
    task::find_process_id_by_name(name).map(|pid| pid.as_u64()).unwrap_or(0)
}
