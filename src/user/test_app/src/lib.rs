#![no_std]

//! SwiftCoreユーザーランドライブラリ

// TODO: いずれRustのstdを使えるようにする！あとRustツールチェーンにも組み込んでもらえるように頑張る

use core::arch::asm;

/// システムコール番号
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallNumber {
    Yield = 1,
    GetTicks = 2,
    IpcSend = 3,
    IpcRecv = 4,
    Exit = 5,
    Write = 6,
}

/// システムコールを呼び出す（引数0個）
#[inline(always)]
pub fn syscall0(num: u64) -> u64 {
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

/// システムコールを呼び出す（引数1個）
#[inline(always)]
pub fn syscall1(num: u64, arg0: u64) -> u64 {
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

/// システムコールを呼び出す（引数2個）
#[inline(always)]
pub fn syscall2(num: u64, arg0: u64, arg1: u64) -> u64 {
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

/// システムコールを呼び出す（引数3個）
#[inline(always)]
pub fn syscall3(num: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            options(nostack, preserves_flags)
        );
    }
    ret
}

/// CPUを他のタスクに譲る
#[inline]
pub fn yield_now() {
    syscall0(SyscallNumber::Yield as u64);
}

/// タイマーティック数を取得
#[inline]
pub fn get_ticks() -> u64 {
    syscall0(SyscallNumber::GetTicks as u64)
}

/// プロセスを終了
#[inline]
pub fn exit(code: u64) -> ! {
    syscall1(SyscallNumber::Exit as u64, code);
    loop {
        unsafe { asm!("hlt") }
    }
}

/// 文字列を出力（TODO: writeシステムコール実装後に有効化）
#[inline]
pub fn write(fd: u64, buf: &[u8]) -> u64 {
    syscall3(
        SyscallNumber::Write as u64,
        fd,
        buf.as_ptr() as u64,
        buf.len() as u64,
    )
}
