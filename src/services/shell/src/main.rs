#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

const SYS_CONSOLE_WRITE: u64 = 5;
const SYS_INITFS_READ: u64 = 6;
const SYS_EXIT: u64 = 7;
const SYS_IPC_RECV: u64 = 4;
const EAGAIN: u64 = u64::MAX - 2;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    write_str("SwiftCore shell\n");
    write_str("Type: (keyboard via IPC)\n");

    let mut buf = [0u8; 128];
    let read = syscall4(
        SYS_INITFS_READ,
        "/etc/motd".as_ptr() as u64,
        "/etc/motd".len() as u64,
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
    );

    if read > 0 && read <= buf.len() as u64 {
        if let Ok(text) = core::str::from_utf8(&buf[..read as usize]) {
            write_str(text);
            write_str("\n");
        }
    }

    loop {
        let mut sender = 0u64;
        let ch = syscall1(SYS_IPC_RECV, &mut sender as *mut u64 as u64);
        if ch == EAGAIN {
            unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)); }
            continue;
        }

        let mut byte = ch as u8;
        if byte == b'\r' {
            byte = b'\n';
        }
        let buf = [byte];
        let _ = syscall2(SYS_CONSOLE_WRITE, buf.as_ptr() as u64, 1);
    }
}

fn write_str(s: &str) {
    let _ = syscall2(SYS_CONSOLE_WRITE, s.as_ptr() as u64, s.len() as u64);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("shell panic\n");
    let _ = syscall1(SYS_EXIT, 1);
    loop {
        unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)); }
    }
}

#[inline(always)]
fn syscall1(num: u64, arg0: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            in("rdi") arg0,
            options(nostack, preserves_flags)
        );
    }
    ret
}

#[inline(always)]
fn syscall2(num: u64, arg0: u64, arg1: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            in("rdi") arg0,
            in("rsi") arg1,
            options(nostack, preserves_flags)
        );
    }
    ret
}

#[inline(always)]
fn syscall4(num: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            in("r10") arg3,
            options(nostack, preserves_flags)
        );
    }
    ret
}
