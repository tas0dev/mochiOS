//! ファイルシステム関連のシステムコール

use super::types::{ENOSYS, EBADF, SUCCESS};

/// Openシステムコール（未実装）
///
/// ファイルを開く
///
/// # 引数
/// - `_path_ptr`: ファイルパスへのポインタ
/// - `_flags`: オープンフラグ
///
/// # 戻り値
/// ファイルディスクリプタ、またはエラーコード
pub fn open(_path_ptr: u64, _flags: u64) -> u64 {
    // TODO: ファイルシステムを実装後に対応
    ENOSYS
}

/// Closeシステムコール（未実装）
///
/// ファイルを閉じる
///
/// # 引数
/// - `_fd`: ファイルディスクリプタ
///
/// # 戻り値
/// 成功時はSUCCESS、エラー時はエラーコード
pub fn close(_fd: u64) -> u64 {
    // TODO: ファイルシステムを実装後に対応
    if _fd < 3 {
        // stdin/stdout/stderr は閉じられない
        EBADF
    } else {
        ENOSYS
    }
}

/// Seekシステムコール（未実装）
///
/// ファイルの読み書き位置を変更する
///
/// # 引数
/// - `_fd`: ファイルディスクリプタ
/// - `_offset`: オフセット
/// - `_whence`: 基準位置 (0=SEEK_SET, 1=SEEK_CUR, 2=SEEK_END)
///
/// # 戻り値
/// 新しいファイル位置、またはエラーコード
pub fn seek(_fd: u64, _offset: i64, _whence: u64) -> u64 {
    // TODO: ファイルシステムを実装後に対応
    ENOSYS
}

/// Fstatシステムコール（未実装）
///
/// ファイルの情報を取得する
pub fn fstat(_fd: u64, _stat_ptr: u64) -> u64 {
    ENOSYS
}

/// Statシステムコール（未実装）
///
/// ファイルの情報を取得する
///
/// # 引数
/// - `_path_ptr`: ファイルパスへのポインタ
/// - `_stat_ptr`: stat構造体へのポインタ
///
/// # 戻り値
/// 成功時はSUCCESS、エラー時はエラーコード
pub fn stat(_path_ptr: u64, _stat_ptr: u64) -> u64 {
    // TODO: ファイルシステムを実装後に対応
    ENOSYS
}

/// Mkdirシステムコール（未実装）
///
/// ディレクトリを作成する
///
/// # 引数
/// - `_path_ptr`: ディレクトリパスへのポインタ
/// - `_mode`: パーミッション
///
/// # 戻り値
/// 成功時はSUCCESS、エラー時はエラーコード
pub fn mkdir(_path_ptr: u64, _mode: u64) -> u64 {
    // TODO: ファイルシステムを実装後に対応
    ENOSYS
}

/// Rmdirシステムコール（未実装）
///
/// ディレクトリを削除する
///
/// # 引数
/// - `_path_ptr`: ディレクトリパスへのポインタ
///
/// # 戻り値
/// 成功時はSUCCESS、エラー時はエラーコード
pub fn rmdir(_path_ptr: u64) -> u64 {
    // TODO: ファイルシステムを実装後に対応
    ENOSYS
}

/// Readdirシステムコール（未実装）
///
/// ディレクトリエントリを読み取る
///
/// # 引数
/// - `_fd`: ディレクトリのファイルディスクリプタ
/// - `_buf_ptr`: バッファへのポインタ
/// - `_buf_len`: バッファサイズ
///
/// # 戻り値
/// 読み取ったバイト数、またはエラーコード
pub fn readdir(_fd: u64, _buf_ptr: u64, _buf_len: u64) -> u64 {
    // TODO: ファイルシステムを実装後に対応
    ENOSYS
}

/// Chdirシステムコール（未実装）
///
/// カレントディレクトリを変更する
///
/// # 引数
/// - `_path_ptr`: ディレクトリパスへのポインタ
///
/// # 戻り値
/// 成功時はSUCCESS、エラー時はエラーコード
pub fn chdir(_path_ptr: u64) -> u64 {
    // TODO: ファイルシステムを実装後に対応
    ENOSYS
}

/// Getcwdシステムコール（簡易実装）
///
/// カレントディレクトリを取得する
///
/// # 引数
/// - `buf_ptr`: バッファへのポインタ
/// - `size`: バッファサイズ
///
/// # 戻り値
/// 成功時はbuf_ptr、エラー時はエラーコード
pub fn getcwd(buf_ptr: u64, size: u64) -> u64 {
    if buf_ptr == 0 || size == 0 {
        return super::types::EINVAL;
    }
    // 暫定実装: "/" を返す
    let cwd = b"/\0";
    if (size as usize) < cwd.len() {
        return super::types::EINVAL;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(cwd.as_ptr(), buf_ptr as *mut u8, cwd.len());
    }
    buf_ptr
}

