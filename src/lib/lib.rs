#![no_std]
#![feature(format_args_nl)]
#![feature(alloc_error_handler)]
pub use core::*;
extern crate alloc as alloc_crate;
pub use alloc_crate::{vec, vec::Vec, string::String, boxed::Box};

pub mod sys;
pub mod io;
pub mod process;
pub mod thread;
pub mod fs;
pub mod heap;
pub mod ipc;

#[alloc_error_handler]
pub fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout)
}

pub use io::{_print};