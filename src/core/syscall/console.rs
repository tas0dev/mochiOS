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
    if !crate::syscall::validate_user_ptr(buf_ptr, len as u64) {
        return EFAULT;
    }

    let mut ok = true;
    crate::syscall::with_user_memory_access(|| unsafe {
        let bytes = core::slice::from_raw_parts(buf_ptr as *const u8, len);
        let text = match core::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                ok = false;
                return;
            }
        };
        util::console::print(format_args!("{}", text));
        util::vga::print(format_args!("{}", text));
    });
    if ok {
        len as u64
    } else {
        EINVAL
    }
}
