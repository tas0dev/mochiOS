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
	/// コンソールへ書き込み (arg0=buf_ptr, arg1=len)
	ConsoleWrite = 5,
	/// initfs 読み込み (arg0=path_ptr, arg1=path_len, arg2=buf_ptr, arg3=buf_len)
	InitfsRead = 6,
	/// 現在のスレッドを終了 (arg0=exit_code)
	Exit = 7,
	/// キーボード1文字読み取り
	KeyboardRead = 8,
	/// 現在のスレッドIDを取得
	GetThreadId = 9,
	/// スレッド名からIDを取得 (arg0=name_ptr, arg1=name_len)
	GetThreadIdByName = 10,
}

/// 未実装エラー
pub const ENOSYS: u64 = u64::MAX;
/// 無効な引数
pub const EINVAL: u64 = u64::MAX - 1;
/// 受信/送信できない（キュー空/満杯）
pub const EAGAIN: u64 = u64::MAX - 2;
/// ファイル/エントリが見つからない
pub const ENOENT: u64 = u64::MAX - 3;
/// 入力が空
pub const ENODATA: u64 = u64::MAX - 4;
