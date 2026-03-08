use alloc::vec::Vec;
use crate::{
    syscall::{EFAULT, EINVAL},
    util,
};

/// コンソール書き込み (buf_ptr, len)
pub fn write(buf_ptr: u64, len: u64) -> u64 {
    if buf_ptr == 0 {
        return EINVAL;
    }
    let len = len as usize;
    if len == 0 {
        return 0;
    }

    let mut copied = Vec::with_capacity(len);
    copied.resize(len, 0);
    if let Err(err) = crate::syscall::copy_from_user(buf_ptr, &mut copied) {
        return err;
    }
    let text = match core::str::from_utf8(&copied) {
        Ok(s) => s,
        Err(_) => return EINVAL,
    };
    util::console::print(format_args!("{}", text));
    util::vga::print(format_args!("{}", text));
    len as u64
}
