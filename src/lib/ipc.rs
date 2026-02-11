use crate::sys::{syscall2, syscall3, SyscallNumber};

pub fn send(dest_pid: u64, data: &[u8]) -> Result<(), u64> {
    let ret = unsafe {
        syscall3(
            SyscallNumber::IpcSend as u64,
            dest_pid,
            data.as_ptr() as u64,
            data.len() as u64,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret)
    }
}

pub fn recv(buf: &mut [u8]) -> (u64, usize) {
    let ret = unsafe {
        syscall2(
            SyscallNumber::IpcRecv as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };

    // エラーの場合
    if ret >= 0xffffffffffffff00 {
        return (0, 0);
    }

    let sender = ret >> 32;
    let len = (ret & 0xffffffff) as usize;
    (sender, len)
}

