//! 入力注入系システムコール（ユーザー側）

use super::sys::{syscall1, SyscallNumber, EINVAL, EPERM};

/// raw スキャンコードを入力キューへ注入する（Service/Core専用）
#[inline]
pub fn inject_scancode(scancode: u8) -> Result<(), u64> {
    let ret = syscall1(SyscallNumber::KeyboardInject as u64, scancode as u64);
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// 3バイトマウスパケットを入力キューへ注入する（Service/Core専用）
///
/// `buttons`: bit0=Left, bit1=Right, bit2=Middle
#[inline]
pub fn inject_mouse_packet(buttons: u8, dx: i8, dy: i8) -> Result<(), u64> {
    let mut status = buttons & 0x07;
    status |= 0x08;
    if dx < 0 {
        status |= 1 << 4;
    }
    if dy < 0 {
        status |= 1 << 5;
    }
    let packet = u64::from(status) | (u64::from(dx as u8) << 8) | (u64::from(dy as u8) << 16);
    let ret = syscall1(SyscallNumber::MouseInject as u64, packet);
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

/// 失敗理由が権限不足/引数不正かを簡易判定する補助
#[inline]
pub fn is_permission_or_arg_error(err: u64) -> bool {
    err == EPERM || err == EINVAL
}
