#![no_std]

extern crate alloc;

use alloc::alloc::{GlobalAlloc, Layout};

/// C関数ラッパーとポートI/O
pub mod cfunc;
/// システムコールの共通インターフェース
pub mod sys;
/// CランタイムとNewlibサポート
pub mod newlib;
/// ipc関連のシステムコール
pub mod ipc;
/// タスク関連のシステムコール
pub mod task;
/// 時間関連のシステムコール
pub mod time;
/// 入出力関連のシステムコール
pub mod io;
/// プロセス管理関連のシステムコール
pub mod process;
/// ファイルシステム関連のシステムコール
pub mod fs;
/// ポートI/O関連のシステムコール
pub mod port;
/// libcのC関数
pub mod libc;
/// Linux/POSIX 互換スタブ (std リンク用)
pub mod posix_stubs;

use core::panic::PanicInfo;
use crate::libc::*;
use crate::sys::SyscallNumber;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
       core::arch::asm!(
           "int 0x80",
           in("rax") SyscallNumber::ExitGroup as u64,
           in("rdi") 1u64,
           options(nostack, noreturn)
       )
    }
}

struct NewlibAllocator;

unsafe impl GlobalAlloc for NewlibAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        libc::memalign(layout.align(), layout.size())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        libc::free(ptr);
    }

    unsafe fn realloc(&self, ptr: *mut u8, _layout: Layout, new_size: usize) -> *mut u8 {
        libc::realloc(ptr, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: NewlibAllocator = NewlibAllocator;

