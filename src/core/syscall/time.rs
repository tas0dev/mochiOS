//! 時間関連システムコール

use super::types::{EAGAIN, EFAULT, EINVAL, SUCCESS};
use crate::interrupt::spinlock::SpinLock;
use crate::task::ThreadId;

#[derive(Clone, Copy)]
struct SleepEntry {
    tid: ThreadId,
    wake_tick: u64,
}

const MAX_SLEEPERS: usize = crate::task::ThreadQueue::MAX_THREADS;
static SLEEP_QUEUE: SpinLock<[Option<SleepEntry>; MAX_SLEEPERS]> =
    SpinLock::new([None; MAX_SLEEPERS]);

fn register_sleep_entry(tid: ThreadId, wake_tick: u64) -> bool {
    let mut queue = SLEEP_QUEUE.lock();

    for slot in queue.iter_mut() {
        if slot.is_some_and(|entry| entry.tid == tid) {
            *slot = Some(SleepEntry { tid, wake_tick });
            return true;
        }
    }

    for slot in queue.iter_mut() {
        if slot.is_none() {
            *slot = Some(SleepEntry { tid, wake_tick });
            return true;
        }
    }

    false
}

pub fn wake_due_sleepers(now_tick: u64) {
    let mut wake_list = [None; MAX_SLEEPERS];
    let mut wake_count = 0usize;

    {
        let mut queue = SLEEP_QUEUE.lock();
        for slot in queue.iter_mut() {
            if let Some(entry) = *slot {
                if now_tick >= entry.wake_tick {
                    if wake_count < wake_list.len() {
                        wake_list[wake_count] = Some(entry.tid);
                        wake_count += 1;
                    }
                    *slot = None;
                }
            }
        }
    }

    for tid in wake_list.iter().take(wake_count).flatten() {
        crate::task::wake_thread(*tid);
    }
}

/// GetTicksシステムコール
///
/// カーネル起動からのティック数を取得
///
/// # 戻り値
/// ティック数
pub fn get_ticks() -> u64 {
    crate::interrupt::timer::get_ticks()
}

/// clock_gettimeシステムコール (Linux互換)
///
/// # 引数
/// - `clk_id`: クロックID (0=CLOCK_REALTIME, 1=CLOCK_MONOTONIC)
/// - `ts_ptr`: timespec構造体へのポインタ
///
/// # 戻り値
/// 成功時は0
pub fn clock_gettime(clk_id: u64, ts_ptr: u64) -> u64 {
    const CLOCK_REALTIME: u64 = 0;
    const CLOCK_MONOTONIC: u64 = 1;
    const CLOCK_PROCESS_CPUTIME_ID: u64 = 2;
    const CLOCK_THREAD_CPUTIME_ID: u64 = 3;

    if ts_ptr == 0 {
        return EINVAL;
    }
    // ユーザー空間アドレスの有効性を検証する (timespec = 16バイト)
    if !crate::syscall::validate_user_ptr(ts_ptr, 16) {
        return EFAULT;
    }

    // タイマーティックを使って時刻を計算 (1ティック = 10ms)
    let ticks = get_ticks();
    let sec = ticks / 100;
    let nsec = (ticks % 100) * 10_000_000;

    match clk_id {
        CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            // timespec { tv_sec: i64, tv_nsec: i64 }
            let mut buf = [0u8; 16];
            buf[0..8].copy_from_slice(&(sec as i64).to_ne_bytes());
            buf[8..16].copy_from_slice(&(nsec as i64).to_ne_bytes());
            match crate::syscall::copy_to_user(ts_ptr, &buf) {
                Ok(()) => SUCCESS,
                Err(e) => e,
            }
        }
        _ => EINVAL,
    }
}

/// SleepUntilシステムコール
///
/// 指定されたティック数まで待機する
///
/// # 引数
/// - `ticks`: 待機する絶対ティック数
///
/// # 戻り値
/// 成功時は0
pub fn sleep_until(ticks: u64) -> u64 {
    if get_ticks() >= ticks {
        return SUCCESS;
    }

    let current_tid = match crate::task::current_thread_id() {
        Some(tid) => tid,
        None => return EINVAL,
    };

    let queued = x86_64::instructions::interrupts::without_interrupts(|| {
        if !register_sleep_entry(current_tid, ticks) {
            return false;
        }
        crate::task::sleep_thread(current_tid);
        true
    });
    if !queued {
        return EAGAIN;
    }

    while get_ticks() < ticks {
        crate::task::yield_now();
    }
    SUCCESS
}
