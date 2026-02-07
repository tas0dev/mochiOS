//! I/O関連のシステムコール

use crate::util::console;
use core::fmt::Write;
use core::slice;
use super::types::{EBADF, EFAULT, SUCCESS};

/// 標準出力のファイルディスクリプタ
const STDOUT_FD: u64 = 1;
/// 標準エラー出力のファイルディスクリプタ  
const STDERR_FD: u64 = 2;

/// Writeシステムコール
///
/// # 引数
/// - `fd`: ファイルディスクリプタ (1=stdout, 2=stderr)
/// - `buf_ptr`: 書き込むデータのポインタ
/// - `len`: 書き込むデータの長さ
///
/// # 戻り値
/// 書き込んだバイト数、またはエラーコード
pub fn write(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    use crate::debug;

    debug!("write: fd={}, buf_ptr={:#x}, len={}", fd, buf_ptr, len);

    // ファイルディスクリプタの検証
    if fd != STDOUT_FD && fd != STDERR_FD {
        debug!("write: invalid fd");
        return EBADF;
    }

    // 長さが0の場合は何もせず成功
    if len == 0 {
        debug!("write: len=0, returning success");
        return SUCCESS;
    }

    // ポインタの検証（NULL チェック）
    if buf_ptr == 0 {
        debug!("write: null pointer");
        return EFAULT;
    }

    // バッファを安全にスライスとして扱う
    // TODO: ユーザー空間のアドレスが有効か適切に検証する
    let buf = unsafe {
        debug!("write: creating slice from {:#x}, len={}", buf_ptr, len);
        slice::from_raw_parts(buf_ptr as *const u8, len as usize)
    };

    debug!("write: successfully created slice, first byte={:#x}", buf[0]);

    // UTF-8として解釈を試みる
    if let Ok(s) = core::str::from_utf8(buf) {
        debug!("write: valid UTF-8: {:?}", s);
        // シリアルポートに文字列を出力
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut console = console::SERIAL.lock();
            let _ = console.write_str(s);
        });
    } else {
        debug!("write: invalid UTF-8, writing bytes");
        // UTF-8でない場合はバイト列として出力
        for &byte in buf {
            x86_64::instructions::interrupts::without_interrupts(|| {
                let mut console = console::SERIAL.lock();
                console.send_byte(byte);
            });
        }
    }

    debug!("write: returning {}", len);
    // 書き込んだバイト数を返す
    len
}

/// Readシステムコール（現在は未実装）
///
/// # 引数
/// - `_fd`: ファイルディスクリプタ
/// - `_buf_ptr`: 読み込むバッファのポインタ
/// - `_len`: 読み込むバッファの長さ
///
/// # 戻り値
/// 読み込んだバイト数、またはエラーコード
pub fn read(_fd: u64, _buf_ptr: u64, _len: u64) -> u64 {
    // TODO: キーボード入力やその他の入力ソースからの読み込みを実装
    super::types::ENOSYS
}
