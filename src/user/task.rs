//! タスク系システムコール（ユーザー側）

use super::sys::{syscall0, syscall1, SyscallNumber};

/// スケジューラに実行権を譲る
pub fn yield_now() {
    let _ = syscall0(SyscallNumber::Yield as u64);
}

/// 現在のプロセスIDを取得
pub fn getpid() -> u64 {
    syscall0(SyscallNumber::GetPid as u64)
}

/// 現在のスレッドIDを取得
pub fn gettid() -> u64 {
    syscall0(SyscallNumber::GetTid as u64)
}

/// 指定されたミリ秒数の間スリープする
pub fn sleep(milliseconds: u64) {
    let _ = syscall1(SyscallNumber::Sleep as u64, milliseconds);
}

/// プロセスをフォークする（未実装）
pub fn fork() -> i64 {
    let ret = syscall0(SyscallNumber::Fork as u64);
    if ret == u64::MAX {
        -1
    } else {
        ret as i64
    }
}

/// 子プロセスの終了を待つ（未実装）
pub fn wait(pid: i64) -> (i64, i32) {
    let ret = syscall1(SyscallNumber::Wait as u64, pid as u64);
    if ret == u64::MAX {
        (-1, 0)
    } else {
        // TODO: ステータスを適切に返す
        (ret as i64, 0)
    }
}

/// プロセスを終了する
pub fn exit(code: i32) -> ! {
    let _ = syscall1(SyscallNumber::Exit as u64, code as u64);
    loop {
        core::hint::spin_loop();
    }
}

