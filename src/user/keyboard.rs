//! キーボード系システムコール（ユーザー側）

use super::sys::{syscall0, SyscallNumber, ENODATA};

/// PS/2 rawスキャンコードを1バイト読み取り（なければ None）
/// 変換はユーザー空間で行う
pub fn read_scancode() -> Option<u8> {
    let ret = syscall0(SyscallNumber::KeyboardRead as u64);
    if ret == ENODATA {
        None
    } else {
        Some(ret as u8)
    }
}
