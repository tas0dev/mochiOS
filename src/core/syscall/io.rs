//! I/O関連のシステムコール

use super::types::{EBADF, EFAULT, SUCCESS};
use crate::util::console;
use crate::util::log::set_level;
use crate::MemoryType::KernelStack;
use crate::{debug, error, info, warn, Kernel};
use alloc::vec::Vec;
use core::fmt::Write;

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

    let mut buf = alloc::vec![0; len as usize];
    if let Err(err) = crate::syscall::copy_from_user(buf_ptr, &mut buf) {
        debug!("write: invalid user ptr {:#x}", buf_ptr);
        return err;
    }
    debug!("write: copied {} bytes from user buffer", buf.len());

    // UTF-8として解釈を試みる
    if let Ok(s) = core::str::from_utf8(&buf) {
        debug!("write: valid UTF-8: {:?}", s);
        // シリアルポートとフレームバッファの両方に出力
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut console = console::SERIAL.lock();
            let _ = console.write_str(s);
        });
        crate::util::vga::print(format_args!("{}", s));
    } else {
        debug!("write: invalid UTF-8, writing bytes");
        // UTF-8でない場合はバイト列として出力
        for &byte in &buf {
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

/// Readシステムコール
/// - fd == 0 の場合はキーボードから1バイト読み取る（なければ ENODATA を返す）
/// - fd >= 3 の場合は initfs から開かれたファイルを読み取る（fs::read に委譲）
pub fn read(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    use super::types::{EFAULT, ENODATA};

    if buf_ptr == 0 {
        return EFAULT;
    }
    if len == 0 {
        return 0;
    }

    if fd == 0 {
        // キーボードから1文字読み取り
        let ch = crate::syscall::keyboard::read_char();
        if ch == ENODATA {
            return ENODATA;
        }
        // ユーザー空間アドレスの有効性を検証する
        if !super::validate_user_ptr(buf_ptr, 1) {
            return EFAULT;
        }
        // 返された値を1バイトとしてコピー
        crate::syscall::with_user_memory_access(|| unsafe {
            let dst = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, 1);
            dst[0] = ch as u8;
        });
        return 1;
    }

    // その他の FD は fs モジュールに委譲
    crate::syscall::fs::read(fd, buf_ptr, len)
}

/// Logシステムコール
///
/// カーネルログにメッセージを書き込む
/// # 引数
/// msg: メッセージ
/// len: メッセージの長さ
/// level: ログレベル（0=ERROR、1=WARNING、2=INFO、3=DEBUG）
///
/// # 戻り値
/// 成功時はSUCCESS、エラー時はエラーコード
pub fn log(msg: u64, len: u64, level: u64) -> u64 {
    if msg == 0 || len == 0 {
        return super::types::EINVAL;
    }

    let mut copied = alloc::vec![0; len as usize];
    if let Err(err) = crate::syscall::copy_from_user(msg, &mut copied) {
        return err;
    }

    let msg = match core::str::from_utf8(&copied) {
        Ok(s) => s,
        Err(_) => return super::types::EINVAL,
    };

    match level {
        0 => error!("{}", msg),
        1 => warn!("{}", msg),
        2 => info!("{}", msg),
        3 => debug!("{}", msg),
        _ => return super::types::EINVAL,
    }
    SUCCESS
}
