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
    if (ret as i64) < 0 {
        -1
    } else {
        ret as i64
    }
}

/// 子プロセスの終了を待つ
pub fn wait(pid: i64) -> (i64, i32) {
    let ret = syscall1(SyscallNumber::Wait as u64, pid as u64);
    if (ret as i64) < 0 {
        (-1, 0)
    } else {
        (ret as i64, 0)
    }
}

/// 子プロセスの終了を非ブロッキングで確認する（WNOHANG）
/// 戻り値: Some(pid) = 終了済み, None = まだ実行中
pub fn wait_nonblocking(pid: i64) -> Option<i64> {
    use super::sys::syscall3;
    const WNOHANG: u64 = 0x1;
    let ret = syscall3(SyscallNumber::Wait as u64, pid as u64, 0, WNOHANG);
    if (ret as i64) > 0 {
        Some(ret as i64)
    } else {
        None
    }
}

/// プロセスを終了する
pub fn exit(code: i32) -> ! {
    let _ = syscall1(SyscallNumber::Exit as u64, code as u64);
    loop {
        core::hint::spin_loop();
    }
}

/// スレッドIDからプロセスの権限レベルを取得
///
/// # 戻り値
/// 0=Core, 1=Service, 2=User, または u64::MAX (エラー)
pub fn get_thread_privilege(tid: u64) -> u64 {
    syscall1(SyscallNumber::GetThreadPrivilege as u64, tid)
}

/// 名前でプロセスを検索し、そのスレッドIDを返す（見つからなければ None）
pub fn find_process_by_name(name: &str) -> Option<u64> {
    use super::sys::syscall2;
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return None;
    }
    let ret = syscall2(
        SyscallNumber::FindProcessByName as u64,
        bytes.as_ptr() as u64,
        bytes.len() as u64,
    );
    if ret == 0 {
        None
    } else {
        Some(ret)
    }
}
