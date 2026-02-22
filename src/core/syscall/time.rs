//! 時間関連システムコール

use super::types::{SUCCESS, EINVAL};

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

    // タイマーティックを使って時刻を計算 (1ティック = 1ms と仮定)
    let ticks = get_ticks();
    let sec = ticks / 1000;
    let nsec = (ticks % 1000) * 1_000_000;

    match clk_id {
        CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            // timespec { tv_sec: i64, tv_nsec: i64 }
            unsafe {
                core::ptr::write(ts_ptr as *mut i64, sec as i64);
                core::ptr::write((ts_ptr + 8) as *mut i64, nsec as i64);
            }
            SUCCESS
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
    let current_ticks = get_ticks();
    if ticks > current_ticks {
        let wait_ticks = ticks - current_ticks;
        if let Some(tid) = crate::task::current_thread_id() {
            crate::task::sleep_thread(tid);
            for _ in 0..wait_ticks.min(1000) {
                crate::task::yield_now();
            }
            crate::task::wake_thread(tid);
        }
    }
    0
}

