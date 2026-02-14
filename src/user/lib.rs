#![no_std]
#![feature(alloc_error_handler)]

extern crate alloc;

use alloc::alloc::{GlobalAlloc, Layout};

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
pub mod cfunc;

use core::panic::PanicInfo;
use cfunc::*;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // TODO: 今後改修する
    unsafe {
       // 強制終了
       let sys_exit = 6;
       core::arch::asm!(
           "int 0x80",
           in("rax") sys_exit,
           in("rdi") 1,
           options(nostack, noreturn)
       )
    }
}

struct NewlibAllocator;

unsafe impl GlobalAlloc for NewlibAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        memalign(layout.align(), layout.size())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        free(ptr);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
         if layout.align() > 8 {
             let new_ptr = memalign(layout.align(), new_size);
             if !new_ptr.is_null() && !ptr.is_null() {
                 let old_size = layout.size();
                 let copy_size = if old_size < new_size { old_size } else { new_size };
                 core::ptr::copy_nonoverlapping(ptr, new_ptr, copy_size);
                 free(ptr);
             }
             new_ptr
         } else {
             realloc(ptr, new_size)
         }
    }
}

#[global_allocator]
static ALLOCATOR: NewlibAllocator = NewlibAllocator;
