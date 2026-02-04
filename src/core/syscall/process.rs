//! プロセス管理関連のシステムコール

use crate::task::{exit_current_task, current_thread_id};
use super::types::{SUCCESS, ENOSYS};

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

/// Forkシステムコール（未実装）
///
/// 現在のプロセスをフォークする
///
/// # 戻り値
/// 親プロセスでは子プロセスのPID、子プロセスでは0、エラー時はENOSYS
pub fn fork() -> u64 {
    // TODO: プロセスのフォークを実装
    ENOSYS
}

/// Waitシステムコール（未実装）
///
/// 子プロセスの終了を待つ
///
/// # 引数
/// - `_pid`: 待機するプロセスID（-1=任意の子プロセス）
/// - `_status_ptr`: 終了ステータスを格納するポインタ
///
/// # 戻り値
/// 終了した子プロセスのPID、またはエラーコード
pub fn wait(_pid: u64, _status_ptr: u64) -> u64 {
    // TODO: プロセスの待機を実装
    ENOSYS
}

