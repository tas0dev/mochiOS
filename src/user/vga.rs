//! フレームバッファアクセス

use crate::sys::{syscall0, syscall1, SyscallNumber};

/// フレームバッファ情報
#[repr(C)]
pub struct FbInfo {
    pub width: u32,
    pub height: u32,
    /// 1行あたりの u32 ピクセル数
    pub stride: u32,
    pub _pad: u32,
}

/// フレームバッファ情報を取得する
pub fn get_info() -> Option<FbInfo> {
    let mut info = FbInfo { width: 0, height: 0, stride: 0, _pad: 0 };
    let ret = syscall1(
        SyscallNumber::GetFramebufferInfo as u64,
        &raw mut info as u64,
    );
    if ret == 0 { Some(info) } else { None }
}

/// フレームバッファをプロセスのアドレス空間にマップし、
/// `*mut u32` ピクセルバッファへのポインタを返す
pub fn map_framebuffer() -> Option<*mut u32> {
    let addr = syscall0(SyscallNumber::MapFramebuffer as u64);
    if addr == 0 || (addr as i64) < 0 {
        None
    } else {
        Some(addr as *mut u32)
    }
}
