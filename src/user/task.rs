//! タスク系システムコール（ユーザー側）

use super::sys::{syscall0, syscall2, SyscallNumber};

/// スケジューラに実行権を譲る
pub fn yield_now() {
    let _ = syscall0(SyscallNumber::Yield as u64);
}

/// 現在のスレッドを終了
pub fn exit(code: u64) -> u64 {
    super::sys::syscall1(SyscallNumber::Exit as u64, code)
}

/// 現在のスレッドIDを取得
pub fn current_thread_id() -> u64 {
    syscall0(SyscallNumber::GetThreadId as u64)
}

/// スレッド名からIDを取得
pub fn thread_id_by_name(name: &str) -> u64 {
    syscall2(
        SyscallNumber::GetThreadIdByName as u64,
        name.as_ptr() as u64,
        name.len() as u64,
    )
}
