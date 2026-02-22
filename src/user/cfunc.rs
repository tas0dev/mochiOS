//! C関数ラッパーとポートI/Oユーティリティ

use core::ffi::c_char;

extern "C" {
    pub fn printf(fmt: *const c_char, ...) -> i32;
    pub fn malloc(size: usize) -> *mut u8;
    pub fn free(ptr: *mut u8);
    pub fn memset(ptr: *mut u8, val: i32, len: usize) -> *mut u8;
    pub fn memcpy(dst: *mut u8, src: *const u8, len: usize) -> *mut u8;
    pub fn strlen(s: *const c_char) -> usize;
}

/// x86 I/Oポートから1バイト読み込む
#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack));
    val
}

/// x86 I/Oポートに1バイト書き込む
#[inline(always)]
pub unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

/// x86 I/Oポートから2バイト読み込む
#[inline(always)]
pub unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    core::arch::asm!("in ax, dx", out("ax") val, in("dx") port, options(nomem, nostack));
    val
}

/// x86 I/Oポートに2バイト書き込む
#[inline(always)]
pub unsafe fn outw(port: u16, val: u16) {
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack));
}

/// x86 I/Oポートから4バイト読み込む
#[inline(always)]
pub unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    core::arch::asm!("in eax, dx", out("eax") val, in("dx") port, options(nomem, nostack));
    val
}

/// x86 I/Oポートに4バイト書き込む
#[inline(always)]
pub unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack));
}
