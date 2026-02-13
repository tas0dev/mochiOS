#![no_std]
#![no_main]

extern crate alloc;
use core::ffi::c_char;
use swiftlib::*;

extern "C" {
    fn printf(format: *const c_char, ...) -> i32;
}

#[no_mangle]
pub extern "C" fn main(_argc: i32, _argv: *const *const u8) -> i32 {
    unsafe {
        let msg = b"Hello from Rust Application with libc!\n\0";
        printf(msg.as_ptr() as *const c_char);
    }
    0
}