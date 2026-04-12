//! フレームバッファアクセス

#[cfg(not(feature = "hosted-vga"))]
use crate::sys::{syscall0, syscall1, SyscallNumber};
#[cfg(feature = "hosted-vga")]
use alloc::vec;
#[cfg(feature = "hosted-vga")]
use alloc::vec::Vec;
#[cfg(feature = "hosted-vga")]
use alloc::format;

#[cfg(feature = "hosted-vga")]
extern crate std;
#[cfg(feature = "hosted-vga")]
use std::fs::File;
#[cfg(feature = "hosted-vga")]
use std::io::Write;
#[cfg(feature = "hosted-vga")]
use std::sync::Mutex;

/// フレームバッファ情報
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FbInfo {
    pub width: u32,
    pub height: u32,
    /// 1行あたりの u32 ピクセル数
    pub stride: u32,
    pub _pad: u32,
}

#[cfg(feature = "hosted-vga")]
struct HostFramebuffer {
    info: FbInfo,
    pixels: Vec<u32>,
}

#[cfg(feature = "hosted-vga")]
static HOST_FB: Mutex<Option<HostFramebuffer>> = Mutex::new(None);

/// フレームバッファ情報を取得する（mochiOS）
#[cfg(not(feature = "hosted-vga"))]
pub fn get_info() -> Option<FbInfo> {
    let mut info = FbInfo { width: 0, height: 0, stride: 0, _pad: 0 };
    let ret = syscall1(
        SyscallNumber::GetFramebufferInfo as u64,
        &raw mut info as u64,
    );
    if ret == 0 { Some(info) } else { None }
}

/// フレームバッファ情報を取得する（hosted-vga）
#[cfg(feature = "hosted-vga")]
pub fn get_info() -> Option<FbInfo> {
    HOST_FB.lock().ok().and_then(|g| g.as_ref().map(|host| host.info))
}

/// フレームバッファをプロセスのアドレス空間にマップし、
/// `*mut u32` ピクセルバッファへのポインタを返す
#[cfg(not(feature = "hosted-vga"))]
pub fn map_framebuffer() -> Option<*mut u32> {
    let addr = syscall0(SyscallNumber::MapFramebuffer as u64);
    if addr == 0 || (addr as i64) < 0 {
        None
    } else {
        Some(addr as *mut u32)
    }
}

#[cfg(feature = "hosted-vga")]
pub fn map_framebuffer() -> Option<*mut u32> {
    let mut guard = HOST_FB.lock().ok()?;
    guard.as_mut().map(|host| host.pixels.as_mut_ptr())
}

/// カーネルコンソールのカーソルをシェルのピクセルY位置に同期する
#[cfg(not(feature = "hosted-vga"))]
pub fn set_console_cursor(pixel_y: u32) {
    syscall1(SyscallNumber::SetConsoleCursor as u64, pixel_y as u64);
}

#[cfg(feature = "hosted-vga")]
pub fn set_console_cursor(pixel_y: u32) {
    let _ = pixel_y;
}

/// カーネルコンソールのカーソルの現在ピクセルY位置を取得する
#[cfg(not(feature = "hosted-vga"))]
pub fn get_console_cursor() -> u32 {
    syscall0(SyscallNumber::GetConsoleCursor as u64) as u32
}

#[cfg(feature = "hosted-vga")]
pub fn get_console_cursor() -> u32 {
    0
}

/// Linuxホスト上のソフトウェアフレームバッファを初期化する。
/// `hosted-vga` 有効時のみ利用可能。
#[cfg(feature = "hosted-vga")]
pub fn host_init_framebuffer(width: u32, height: u32) -> Result<(), &'static str> {
    if width == 0 || height == 0 {
        return Err("invalid framebuffer size");
    }
    let len = (width as usize).saturating_mul(height as usize);
    let info = FbInfo {
        width,
        height,
        stride: width,
        _pad: 0,
    };
    let mut guard = HOST_FB.lock().map_err(|_| "host framebuffer lock poisoned")?;
    *guard = Some(HostFramebuffer {
        info,
        pixels: vec![0xFF00_0000; len],
    });
    Ok(())
}

/// ホストフレームバッファをPPMへ保存する（デバッグ用）。
#[cfg(feature = "hosted-vga")]
pub fn host_dump_ppm(path: &str) -> Result<(), &'static str> {
    let (w, h, pixels) = {
        let guard = HOST_FB.lock().map_err(|_| "host framebuffer lock poisoned")?;
        let host = guard.as_ref().ok_or("host framebuffer is not initialized")?;
        (host.info.width as usize, host.info.height as usize, host.pixels.clone())
    };

    let mut file = File::create(path).map_err(|_| "failed to create ppm file")?;
    file.write_all(format!("P6\n{} {}\n255\n", w, h).as_bytes())
        .map_err(|_| "failed to write ppm header")?;
    let mut rgb = Vec::with_capacity(w.saturating_mul(h).saturating_mul(3));
    for px in pixels {
        rgb.push(((px >> 16) & 0xFF) as u8);
        rgb.push(((px >> 8) & 0xFF) as u8);
        rgb.push((px & 0xFF) as u8);
    }
    file.write_all(&rgb)
        .map_err(|_| "failed to write ppm data")?;
    Ok(())
}
