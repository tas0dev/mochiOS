#![no_std]
#![no_main]

extern crate alloc;
use core::ffi::c_char;
use swiftlib::cfunc::*;

#[no_mangle]
pub extern "C" fn main(argc: i32, argv: *const *const u8) -> i32 {
    unsafe {
        let msg = b"Hello from Rust Application with libc!\n\0";
        printf(msg.as_ptr() as *const c_char);

        printf(b"argc: %d\n\0".as_ptr() as *const c_char, argc);
        for i in 0..argc {
            let arg_ptr = *argv.offset(i as isize);
            printf(b"argv[%d]: %s\n\0".as_ptr() as *const c_char, i, arg_ptr);
        }
    }
    0
}