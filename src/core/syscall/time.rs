//! 時間関連システムコール

use super::types::{EFAULT, EINVAL, SUCCESS};

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
            unsafe {
                core::ptr::write_unaligned(ts_ptr as *mut i64, sec as i64);
                core::ptr::write_unaligned((ts_ptr + 8) as *mut i64, nsec as i64);
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
    while get_ticks() < ticks {
        crate::task::yield_now();
    }
    0
}
