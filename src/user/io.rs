//! I/O関連のシステムコールラッパー

use crate::sys::{syscall1, syscall2, syscall3, SyscallNumber};

/// 標準出力のファイルディスクリプタ
pub const STDOUT: u64 = 1;
/// 標準エラー出力のファイルディスクリプタ
pub const STDERR: u64 = 2;
/// 標準入力のファイルディスクリプタ
pub const STDIN: u64 = 0;

/// ファイルオープンフラグ
pub const O_RDONLY: u64 = 0;
pub const O_WRONLY: u64 = 1;
pub const O_RDWR: u64 = 2;
pub const O_CREAT: u64 = 0x40;
pub const O_TRUNC: u64 = 0x200;
pub const O_APPEND: u64 = 0x400;

/// ファイルディスクリプタに書き込む
///
/// # 引数
/// - `fd`: ファイルディスクリプタ
/// - `buf`: 書き込むデータ
///
/// # 戻り値
/// 書き込んだバイト数、またはエラーコード
#[inline]
pub fn write(fd: u64, buf: &[u8]) -> u64 {
    syscall3(
        SyscallNumber::Write as u64,
        fd,
        buf.as_ptr() as u64,
        buf.len() as u64,
    )
}

/// 標準出力に書き込む
///
/// # 引数
/// - `buf`: 書き込むデータ
///
/// # 戻り値
/// 書き込んだバイト数、またはエラーコード
#[inline]
pub fn write_stdout(buf: &[u8]) -> u64 {
    write(STDOUT, buf)
}

/// 標準エラー出力に書き込む
///
/// # 引数
/// - `buf`: 書き込むデータ
///
/// # 戻り値
/// 書き込んだバイト数、またはエラーコード
#[inline]
pub fn write_stderr(buf: &[u8]) -> u64 {
    write(STDERR, buf)
}

/// 標準出力に文字列を書き込む
///
/// # 引数
/// - `s`: 書き込む文字列
///
/// # 戻り値
/// 書き込んだバイト数、またはエラーコード
#[inline]
pub fn print(s: &str) -> u64 {
    write_stdout(s.as_bytes())
}

/// ファイルディスクリプタから読み込む
///
/// # 引数
/// - `fd`: ファイルディスクリプタ
/// - `buf`: 読み込むバッファ
///
/// # 戻り値
/// 読み込んだバイト数、またはエラーコード
#[inline]
pub fn read(fd: u64, buf: &mut [u8]) -> u64 {
    syscall3(
        SyscallNumber::Read as u64,
        fd,
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
    )
}

/// ファイルを開く（未実装）
///
/// # 引数
/// - `path`: ファイルパス
/// - `flags`: オープンフラグ
///
/// # 戻り値
/// ファイルディスクリプタ、またはエラーコード
#[inline]
pub fn open(path: &str, flags: u64) -> i64 {
    let ret = syscall2(
        SyscallNumber::Open as u64,
        path.as_ptr() as u64,
        flags,
    );
    if ret == u64::MAX {
        -1
    } else {
        ret as i64
    }
}

/// ファイルを閉じる（未実装）
///
/// # 引数
/// - `fd`: ファイルディスクリプタ
///
/// # 戻り値
/// 成功時は0、エラー時は負の値
#[inline]
pub fn close(fd: u64) -> i64 {
    let ret = syscall1(SyscallNumber::Close as u64, fd);
    if ret == u64::MAX {
        -1
    } else {
        ret as i64
    }
}
