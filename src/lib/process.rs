use crate::sys::{syscall0, syscall1, SyscallNumber};

pub fn exit(code: u64) -> ! {
    unsafe {
        syscall1(SyscallNumber::Exit as u64, code);
        loop {
            core::arch::asm!("hlt");
        }
    }
}

pub fn id() -> u64 {
    unsafe {
        syscall0(SyscallNumber::GetPid as u64)
    }
}

pub fn find_by_name(name: &str) -> Option<u64> {
     let ret = unsafe {
        crate::sys::syscall2(
            SyscallNumber::FindProcessByName as u64,
            name.as_ptr() as u64,
            name.len() as u64,
        )
    };
    if ret == 0 {
        None
    } else {
        Some(ret)
    }
}
