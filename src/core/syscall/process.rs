//! プロセス管理関連のシステムコール

use super::types::{EFAULT, EINVAL, ENOMEM, ENOSYS, SUCCESS};
use crate::interrupt::spinlock::SpinLock;
use crate::task::ThreadId;

/// ユーザー空間の上限アドレス (x86-64 canonical hole 下側)
const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;
/// Linux互換: 子プロセスが存在しない
const ECHILD: u64 = (-10i64) as u64;
/// Linux互換: 操作がタイムアウトした
const ETIMEDOUT: u64 = (-110i64) as u64;
/// PIT割り込み周期 (10ms)
const TICK_MS: u64 = 10;
use crate::task::{current_thread_id, exit_current_task};

#[derive(Clone, Copy)]
struct FutexWaitEntry {
    tid: ThreadId,
    uaddr: u64,
    wake_tick: u64,
}

const MAX_FUTEX_WAITERS: usize = crate::task::ThreadQueue::MAX_THREADS;
const NO_TIMEOUT_WAKE_TICK: u64 = u64::MAX;
static FUTEX_WAIT_QUEUE: SpinLock<[Option<FutexWaitEntry>; MAX_FUTEX_WAITERS]> =
    SpinLock::new([None; MAX_FUTEX_WAITERS]);

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

fn register_futex_waiter(tid: ThreadId, uaddr: u64, wake_tick: u64) -> bool {
    let mut queue = FUTEX_WAIT_QUEUE.lock();

    for slot in queue.iter_mut() {
        if slot.is_some_and(|entry| entry.tid == tid) {
            // 1スレッドは同時に1つの futex wait のみ許可する
            return false;
        }
    }

    for slot in queue.iter_mut() {
        if slot.is_none() {
            *slot = Some(FutexWaitEntry {
                tid,
                uaddr,
                wake_tick,
            });
            return true;
        }
    }

    false
}

fn futex_waiter_exists(tid: ThreadId, uaddr: u64) -> bool {
    let queue = FUTEX_WAIT_QUEUE.lock();
    queue
        .iter()
        .flatten()
        .any(|entry| entry.tid == tid && entry.uaddr == uaddr)
}

fn remove_futex_waiter_by_tid(tid: ThreadId) -> bool {
    let mut queue = FUTEX_WAIT_QUEUE.lock();
    for slot in queue.iter_mut() {
        if slot.is_some_and(|entry| entry.tid == tid) {
            *slot = None;
            return true;
        }
    }
    false
}

pub fn clear_futex_waiter(tid: ThreadId) {
    let _ = remove_futex_waiter_by_tid(tid);
}

/// FUTEX_WAIT のタイムアウトに達したスレッドを起床させる（タイマー割り込みから呼ばれる）
pub fn wake_due_futex_waiters(now_tick: u64) {
    let mut wake_list = [None; MAX_FUTEX_WAITERS];
    let mut wake_count = 0usize;

    {
        let mut queue = FUTEX_WAIT_QUEUE.lock();
        for slot in queue.iter_mut() {
            if let Some(entry) = *slot {
                if entry.wake_tick != NO_TIMEOUT_WAKE_TICK && now_tick >= entry.wake_tick {
                    *slot = None;
                    debug_assert!(wake_count < wake_list.len());
                    wake_list[wake_count] = Some(entry.tid);
                    wake_count += 1;
                }
            }
        }
    }

    for tid in wake_list.iter().take(wake_count).flatten() {
        crate::task::with_thread_mut(*tid, |thread| thread.set_futex_timed_out(true));
        crate::task::wake_thread(*tid);
    }
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
        // 既存のヒープページを上書きしないよう、未マップ開始位置から拡張する。
        let start_addr =
            if crate::mem::paging::is_user_range_mapped_in_table(pt_phys, current_brk, 1) {
                current_brk.saturating_add(1)
            } else {
                current_brk
            };
        let start_page = match page_align_up(start_addr) {
            Some(v) => v,
            None => return Err(EINVAL),
        };
        // 一部のユーザーランタイムは brk 境界アドレスにメタデータを書き込むため、
        // `addr` がページ境界ちょうどの場合でもそのページを含めて確保する。
        let map_end = addr.saturating_add(1);
        let end_page = match page_align_up(map_end) {
            Some(v) if is_user_range(v.saturating_sub(1), 1) => v,
            _ => return Err(EINVAL),
        };

        if end_page > start_page {
            let size = end_page - start_page;
            if crate::mem::paging::map_and_copy_segment_to(
                pt_phys,
                start_page,
                0,
                size,
                &[],
                true,
                false,
            )
            .is_err()
            {
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
    let parent_tid = match current_thread_id() {
        Some(tid) => tid,
        None => return ENOSYS,
    };
    let parent_pid = match crate::task::with_thread(parent_tid, |t| t.process_id()) {
        Some(pid) => pid,
        None => return ENOSYS,
    };

    let (parent_priv, parent_priority, parent_pt, heap_start, heap_end) =
        match crate::task::with_process(parent_pid, |p| {
            (
                p.privilege(),
                p.priority(),
                p.page_table(),
                p.heap_start(),
                p.heap_end(),
            )
        }) {
            Some(v) => v,
            None => return ENOSYS,
        };
    let parent_pt = match parent_pt {
        Some(pt) => pt,
        None => return ENOSYS,
    };

    let child_pt = match crate::mem::paging::clone_user_page_table(parent_pt) {
        Ok(pt) => pt,
        Err(_) => return ENOMEM,
    };

    let (user_rip, user_rsp, user_rflags, parent_fs) = crate::task::with_thread(parent_tid, |t| {
        let (rip, rsp, rflags) = t.syscall_user_context();
        (rip, rsp, rflags, t.fs_base())
    })
    .unwrap_or((0, 0, 0, 0));
    if user_rip == 0 || user_rsp == 0 {
        let _ = crate::mem::paging::destroy_user_page_table(child_pt);
        return ENOSYS;
    }

    // 親プロセスの FD テーブルを fork 前にクローンする
    let child_fd_table = crate::task::with_process(parent_pid, |p| p.clone_fd_table_for_fork());

    let mut child_proc =
        crate::task::Process::new("fork", parent_priv, Some(parent_pid), parent_priority);
    child_proc.set_page_table(child_pt);
    child_proc.set_heap_start(heap_start);
    child_proc.set_heap_end(heap_end);
    // 親の FD テーブルを子に継承する
    if let Some(table) = child_fd_table {
        child_proc.set_fd_table(table);
    }
    let child_pid = child_proc.id();
    if crate::task::add_process(child_proc).is_none() {
        let _ = crate::mem::paging::destroy_user_page_table(child_pt);
        return ENOMEM;
    }

    const KERNEL_THREAD_STACK_SIZE: usize = 4096 * 4;
    let kstack = match crate::task::thread::allocate_kernel_stack(KERNEL_THREAD_STACK_SIZE) {
        Some(s) => s,
        None => {
            let _ = crate::task::remove_process(child_pid);
            let _ = crate::mem::paging::destroy_user_page_table(child_pt);
            return ENOMEM;
        }
    };
    let child_thread = crate::task::Thread::new_fork_child(
        child_pid,
        user_rip,
        user_rsp,
        user_rflags,
        parent_fs,
        kstack,
        KERNEL_THREAD_STACK_SIZE,
    );
    if crate::task::add_thread(child_thread).is_none() {
        let _ = crate::task::remove_process(child_pid);
        let _ = crate::mem::paging::destroy_user_page_table(child_pt);
        return ENOMEM;
    }

    child_pid.as_u64()
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

    let target_pid = if pid == -1 {
        None
    } else {
        Some(crate::task::ProcessId::from_u64(pid as u64))
    };

    // POSIX互換の待機: ゾンビを回収、存在しなければブロックまたはWNOHANGで0
    loop {
        if let Some((reaped_pid, exit_code)) =
            crate::task::reap_zombie_child_process(current_pid, target_pid)
        {
            if status_ptr != 0 {
                let status = ((exit_code & 0xff) << 8) as i32;
                crate::syscall::with_user_memory_access(|| unsafe {
                    core::ptr::write_unaligned(status_ptr as *mut i32, status);
                });
            }
            return reaped_pid.as_u64();
        }

        if !crate::task::has_child_process(current_pid, target_pid) {
            return ECHILD;
        }

        if options & WNOHANG != 0 {
            return 0;
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

    let current_tid = match current_thread_id() {
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

        if crate::mem::paging::map_and_copy_segment_to(
            pt_phys,
            map_start,
            0,
            size,
            &[],
            true,
            false,
        )
        .is_err()
        {
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

/// Futexシステムコール
///
/// FUTEX_WAIT / FUTEX_WAKE の待機キュー方式を実装する。
/// timeout は「現在tickからの相対tick」として扱う（0は無期限）。
pub fn futex(uaddr: u64, op: u32, val: u64, timeout: u64) -> u64 {
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
            let current_tid = match current_thread_id() {
                Some(tid) => tid,
                None => return ENOSYS,
            };
            // ユーザー空間アドレスの有効性を検証する
            if !super::validate_user_ptr(uaddr, 4) {
                return EFAULT;
            }
            let wake_tick = if timeout == 0 {
                NO_TIMEOUT_WAKE_TICK
            } else {
                crate::syscall::time::get_ticks().saturating_add(timeout)
            };

            crate::task::with_thread_mut(current_tid, |thread| thread.set_futex_timed_out(false));

            // 割り込み禁止区間内で「値の確認 → キュー登録 → スリープ → 最初のyield」を
            // アトミックに実行することで、wake と sleep の競合ウィンドウを排除する。
            // yield_now() 内部の switch_to_thread も CLI を実行するため、
            // without_interrupts をネストしても安全に動作する。
            let queued = x86_64::instructions::interrupts::without_interrupts(|| {
                let current_val = crate::syscall::with_user_memory_access(|| unsafe {
                    core::ptr::read_volatile(uaddr as *const u32)
                });
                if current_val != val as u32 {
                    return Err(EAGAIN);
                }
                if !register_futex_waiter(current_tid, uaddr, wake_tick) {
                    return Err(EAGAIN);
                }
                crate::task::sleep_thread(current_tid);
                // 割り込み禁止のまま最初のコンテキストスイッチを実行し、
                // sleep_thread とyield の間に wake シグナルが失われる競合を防ぐ。
                crate::task::yield_now();
                Ok(())
            });
            if let Err(err) = queued {
                return err;
            }

            enum WaitResult {
                Continue,
                Success,
                TimedOut,
            }

            // 起床後に条件を確認し、まだ待機が必要な場合のみ再度 yield する。
            loop {
                let result = x86_64::instructions::interrupts::without_interrupts(|| {
                    let timed_out = crate::task::with_thread_mut(current_tid, |thread| {
                        thread.take_futex_timed_out()
                    })
                    .unwrap_or(false);
                    if timed_out {
                        return WaitResult::TimedOut;
                    }

                    if !futex_waiter_exists(current_tid, uaddr) {
                        return WaitResult::Success;
                    }

                    if wake_tick != NO_TIMEOUT_WAKE_TICK
                        && crate::syscall::time::get_ticks() >= wake_tick
                    {
                        if remove_futex_waiter_by_tid(current_tid) {
                            crate::task::with_thread_mut(current_tid, |thread| {
                                thread.set_futex_timed_out(false);
                            });
                            return WaitResult::TimedOut;
                        }
                        let timed_out = crate::task::with_thread_mut(current_tid, |thread| {
                            thread.take_futex_timed_out()
                        })
                        .unwrap_or(false);
                        return if timed_out {
                            WaitResult::TimedOut
                        } else {
                            WaitResult::Success
                        };
                    }

                    WaitResult::Continue
                });

                match result {
                    WaitResult::Continue => crate::task::yield_now(),
                    WaitResult::Success => return SUCCESS,
                    WaitResult::TimedOut => return ETIMEDOUT,
                }
            }
        }
        FUTEX_WAKE => {
            if uaddr == 0 {
                return EFAULT;
            }
            if !super::validate_user_ptr(uaddr, 4) {
                return EFAULT;
            }
            let max_wake = core::cmp::min(val as usize, MAX_FUTEX_WAITERS);
            if max_wake == 0 {
                return 0;
            }

            let mut wake_list = [None; MAX_FUTEX_WAITERS];
            let mut wake_count = 0usize;
            {
                let mut queue = FUTEX_WAIT_QUEUE.lock();
                for slot in queue.iter_mut() {
                    if wake_count >= max_wake {
                        break;
                    }
                    if let Some(entry) = *slot {
                        if entry.uaddr == uaddr {
                            *slot = None;
                            wake_list[wake_count] = Some(entry.tid);
                            wake_count += 1;
                        }
                    }
                }
            }

            for tid in wake_list.iter().take(wake_count).flatten() {
                crate::task::with_thread_mut(*tid, |thread| thread.set_futex_timed_out(false));
                crate::task::wake_thread(*tid);
            }
            wake_count as u64
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
            if addr > USER_SPACE_END {
                return EINVAL;
            }
            // FS ベースレジスタを設定 (WRFSBASE または IA32_FS_BASE MSR)
            unsafe {
                crate::cpu::write_fs_base(addr);
            }
            // 現在のスレッドに FS base を記録 (コンテキストスイッチ時に復元するため)
            if let Some(tid) = current_thread_id() {
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
            crate::syscall::with_user_memory_access(|| unsafe {
                core::ptr::write_unaligned(addr as *mut u64, val)
            });
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

    let mut name_buf = [0u8; 64];
    if crate::syscall::copy_from_user(name_ptr, &mut name_buf[..len as usize]).is_err() {
        return 0;
    }
    let name = match str::from_utf8(&name_buf[..len as usize]) {
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
