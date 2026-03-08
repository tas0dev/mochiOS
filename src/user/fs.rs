//! ファイルシステム関連のシステムコール（ユーザー側）

use super::sys::{syscall1, syscall2, syscall3, SyscallNumber};

fn path_buf(path: &str) -> ([u8; 512], usize) {
    let mut buf = [0u8; 512];
    let bytes = path.as_bytes();
    let len = bytes.len().min(511);
    buf[..len].copy_from_slice(&bytes[..len]);
    (buf, len)
}

pub fn mkdir(path: &str, mode: u32) -> u64 {
    let (buf, _) = path_buf(path);
    syscall2(SyscallNumber::Mkdir as u64, buf.as_ptr() as u64, mode as u64)
}

pub fn rmdir(path: &str) -> u64 {
    let (buf, _) = path_buf(path);
    syscall1(SyscallNumber::Rmdir as u64, buf.as_ptr() as u64)
}

pub fn readdir(fd: u64, buf: &mut [u8]) -> u64 {
    syscall3(
        SyscallNumber::Readdir as u64,
        fd,
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
    )
}

pub fn chdir(path: &str) -> u64 {
    let (buf, _) = path_buf(path);
    syscall1(SyscallNumber::Chdir as u64, buf.as_ptr() as u64)
}
