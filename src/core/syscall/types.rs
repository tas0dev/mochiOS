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
    /// プロセス終了 (arg0=exit_code)
    Exit = 6,
    /// 書き込み (arg0=fd, arg1=buf_ptr, arg2=len)
    Write = 7,
    /// 読み込み (arg0=fd, arg1=buf_ptr, arg2=len)
    Read = 8,
    /// 現在のプロセスIDを取得
    GetPid = 9,
    /// 現在のスレッドIDを取得
    GetTid = 10,
    /// スリープ (arg0=milliseconds)
    Sleep = 11,
    /// ファイルを開く (arg0=path_ptr, arg1=flags)
    Open = 12,
    /// ファイルを閉じる (arg0=fd)
    Close = 13,
    /// Fork (arg0=reserved)
    Fork = 14,
    /// Wait (arg0=pid, arg1=status_ptr)
    Wait = 15,
}

/// 成功
pub const SUCCESS: u64 = 0;
/// 未実装エラー
pub const ENOSYS: u64 = u64::MAX;
/// 無効な引数
pub const EINVAL: u64 = u64::MAX - 1;
/// 受信/送信できない（キュー空/満杯）
pub const EAGAIN: u64 = u64::MAX - 2;
/// 不正なファイルディスクリプタ
pub const EBADF: u64 = u64::MAX - 3;
/// 不正なアドレス
pub const EFAULT: u64 = u64::MAX - 4;
/// ファイルが見つからない
pub const ENOENT: u64 = u64::MAX - 5;
/// 権限エラー
pub const EPERM: u64 = u64::MAX - 6;
