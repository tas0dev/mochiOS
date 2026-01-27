/// READ（読み取ったバイト数）
pub const SYS_READ: u64 = 0;
/// WRITE（書き込んだバイト数）
pub const SYS_WRITE: u64 = 1;
/// OPEN（ファイルディスクリプタ）
pub const SYS_OPEN: u64 = 2;
/// CLOSE（クローズする）
pub const SYS_CLOSE: u64 = 3;
/// STAT（ファイル情報を取得する）
pub const SYS_STAT: u64 = 4;
/// FSTAT（ファイル情報を取得する）
pub const SYS_FSTAT: u64 = 5;
/// LSTAT（シンボリックリンクの情報を取得する）
pub const SYS_LSTAT: u64 = 6;
/// MMAP（メモリマップドファイルをマップする）
pub const SYS_MMAP: u64 = 9;
/// BRK（ヒープ領域の終端を設定する）
pub const SYS_BRK: u64 = 12;
/// ACCESS（ファイルアクセス権を確認する）
pub const SYS_ACCESS: u64 = 21;
/// EXIT（プロセスを終了する）
pub const SYS_EXIT: u64 = 60;
/// GETPID（プロセスIDを取得する）
pub const SYS_GETPID: u64 = 39;
/// GETTID（スレッドIDを取得する）
pub const SYS_GETTID: u64 = 186;