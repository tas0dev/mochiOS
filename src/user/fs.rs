//! ファイルシステム関連のシステムコール（ユーザー側）

use super::sys::{syscall1, syscall2, syscall3, SyscallNumber};
use alloc::vec::Vec;

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

/// カレントワーキングディレクトリを取得する
pub fn getcwd(buf: &mut [u8]) -> Option<&str> {
    let ret = syscall2(
        SyscallNumber::Getcwd as u64,
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
    );
    if ret == 0 || ret > 0xFFFF_FFFF_0000_0000 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    core::str::from_utf8(&buf[..len]).ok()
}

/// 実行ファイルをパス指定で起動する（カーネルの通常 exec 経路）
pub fn exec_via_fs(path: &str) -> Result<u64, i64> {
    crate::process::exec(path).map_err(|_| -2)
}

/// ファイルを開く（通常 open システムコール）
pub fn open_via_fs(path: &str) -> Result<u64, i64> {
    let fd = crate::io::open(path, crate::io::O_RDONLY);
    if fd < 0 {
        Err(-2)
    } else {
        Ok(fd as u64)
    }
}

/// ファイルを読む（通常 read システムコール）
pub fn read_via_fs(fd: u64, out: &mut [u8]) -> Result<usize, i64> {
    let n = crate::io::read(fd, out);
    if (n as i64) < 0 {
        Err(-5)
    } else {
        Ok(n as usize)
    }
}

/// ファイルを閉じる
pub fn close_via_fs(fd: u64) {
    let _ = crate::io::close(fd);
}

/// ファイル全体を読む。存在しない場合は Ok(None)。
pub fn read_file_via_fs(path: &str, max_size: usize) -> Result<Option<Vec<u8>>, i64> {
    let fd = match open_via_fs(path) {
        Ok(fd) => fd,
        Err(errno) if errno == -2 => return Ok(None),
        Err(errno) => return Err(errno),
    };

    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    while out.len() < max_size {
        let to_read = core::cmp::min(chunk.len(), max_size - out.len());
        match read_via_fs(fd, &mut chunk[..to_read]) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(errno) => {
                close_via_fs(fd);
                return Err(errno);
            }
        }
    }
    close_via_fs(fd);
    Ok(Some(out))
}
