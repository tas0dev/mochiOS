//! 時刻系システムコール（ユーザー側）

use super::sys::{syscall0, syscall1, SyscallNumber};

/// タイマーティック数を取得
pub fn get_ticks() -> u64 {
    syscall0(SyscallNumber::GetTicks as u64)
}

/// ミリ秒単位でスリープ
pub fn sleep_ms(ms: u64) {
    syscall1(SyscallNumber::Sleep as u64, ms);
}
