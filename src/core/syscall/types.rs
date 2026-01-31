/// システムコール番号
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallNumber {
    /// スケジューラへ譲る
    Yield = 1,
    /// タイマーティック数を取得
    GetTicks = 2,
    /// IPC送信 (arg0=dest_thread_id, arg1=value)
    IpcSend = 3,
    /// IPC受信 (arg0=sender_ptr)
    IpcRecv = 4,
    /// initfsから実行可能ファイルを読み込み実行 (arg0=filename_ptr)
    Exec = 5,
}

/// 未実装エラー
pub const ENOSYS: u64 = u64::MAX;
/// 無効な引数
pub const EINVAL: u64 = u64::MAX - 1;
/// 受信/送信できない（キュー空/満杯）
pub const EAGAIN: u64 = u64::MAX - 2;
