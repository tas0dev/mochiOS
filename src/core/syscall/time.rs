//! 時間関連システムコール

/// GetTicksシステムコール
///
/// カーネル起動からのティック数を取得
///
/// # 戻り値
/// ティック数
pub fn get_ticks() -> u64 {
    crate::interrupt::timer::get_ticks()
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
            // 簡易的な待機
            for _ in 0..wait_ticks.min(1000) {
                crate::task::yield_now();
            }
            crate::task::wake_thread(tid);
        }
    }
    0
}

