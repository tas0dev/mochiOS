//! ファイルシステムIPC定数（カーネル・サービス・ユーザー空間共通）

/// ファイルパスの最大長
pub const FS_PATH_MAX: usize = 128;

/// 1回のFS応答で送信可能なデータの最大サイズ
pub const FS_DATA_MAX: usize = 2048;

/// IPCメッセージの最大サイズ（将来的な拡張用、現在は使用されていない）
pub const IPC_MAX_MSG_SIZE: usize = 65536;
