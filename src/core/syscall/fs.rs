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
