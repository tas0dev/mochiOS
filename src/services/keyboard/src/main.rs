#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

const SYS_CONSOLE_WRITE: u64 = 5;
const SYS_EXIT: u64 = 7;
const SYS_KEYBOARD_READ: u64 = 8;
const SYS_IPC_SEND: u64 = 3;
const SYS_GET_THREAD_ID_BY_NAME: u64 = 10;
const ENODATA: u64 = u64::MAX - 4;
const EAGAIN: u64 = u64::MAX - 2;
const ENOENT: u64 = u64::MAX - 3;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    write_str("keyboard service started\n");

    let shell_id = loop {
        let id = syscall2(
            SYS_GET_THREAD_ID_BY_NAME,
            "core.service.shell".as_ptr() as u64,
            "core.service.shell".len() as u64,
        );
        if id != ENOENT && id != EAGAIN {
            break id;
        }
        unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)); }
    };

    loop {
        let ch = syscall0(SYS_KEYBOARD_READ);
        if ch == ENODATA {
            unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)); }
            continue;
        }

        let ret = syscall2(SYS_IPC_SEND, shell_id, ch as u64);
        if ret == EAGAIN {
            unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)); }
        }
    }
}

fn write_str(s: &str) {
    let _ = syscall2(SYS_CONSOLE_WRITE, s.as_ptr() as u64, s.len() as u64);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("keyboard service panic\n");
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
fn syscall0(num: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            options(nostack, preserves_flags)
        );
    }
    ret
}
