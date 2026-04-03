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

// --- FS service IPC helpers ---
use crate::ipc;
use crate::task;
use crate::time;
use core::mem::size_of;

use crate::fs_consts::{FS_DATA_MAX, FS_PATH_MAX, IPC_MAX_MSG_SIZE};
const FS_REQ_TIMEOUT_MS: u64 = 2000;

#[repr(C)]
#[derive(Clone, Copy)]
struct FsRequestIp {
    op: u64,
    arg1: u64,
    arg2: u64,
    path: [u8; FS_PATH_MAX],
}

impl FsRequestIp {
    const OP_OPEN: u64 = 1;
    const OP_READ: u64 = 2;
    const OP_CLOSE: u64 = 4;
    const OP_EXEC: u64 = 5;

    fn exec(path: &str) -> Option<Self> {
        let mut path_buf = [0u8; FS_PATH_MAX];
        let bytes = path.as_bytes();
        if bytes.len() >= FS_PATH_MAX {
            return None;
        }
        path_buf[..bytes.len()].copy_from_slice(bytes);
        Some(Self { op: Self::OP_EXEC, arg1: 0, arg2: 0, path: path_buf })
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FsResponseIp {
    pub status: i64,
    pub len: u64,
    pub data: [u8; FS_DATA_MAX],
}

fn find_fs_service() -> Option<u64> {
    task::find_process_by_name("fs.service")
}

fn fs_ipc_request(fs_tid: u64, req: &FsRequestIp) -> Result<FsResponseIp, ()> {
    let req_slice = unsafe {
        core::slice::from_raw_parts(req as *const _ as *const u8, size_of::<FsRequestIp>())
    };
    if ipc::ipc_send(fs_tid, req_slice) != 0 {
        return Err(());
    }

    let mut resp_buf = [0u8; size_of::<FsResponseIp>()];
    let start_tick = time::get_ticks();
    loop {
        let (sender, len) = ipc::ipc_recv(&mut resp_buf);
        if sender == 0 && len == 0 {
            if time::get_ticks().saturating_sub(start_tick) > FS_REQ_TIMEOUT_MS {
                return Err(());
            }
            time::sleep_ms(1);
            continue;
        }
        if sender != fs_tid || (len as usize) < size_of::<FsResponseIp>() {
            continue;
        }
        let resp: FsResponseIp = unsafe { core::ptr::read_unaligned(resp_buf.as_ptr() as *const FsResponseIp) };
        return Ok(resp);
    }
}

/// Execute a file via fs.service. Returns PID on success or negative errno on failure.
pub fn exec_via_fs(path: &str) -> Result<u64, i64> {
    let fs_tid = find_fs_service().ok_or(-3)?; // ESRCH
    let exec_req = FsRequestIp::exec(path).ok_or(-22)?; // EINVAL
    let resp = fs_ipc_request(fs_tid, &exec_req).map_err(|_| -5)?; // EIO
    if resp.status < 0 {
        return Err(resp.status);
    }
    Ok(resp.status as u64)
}

/// Open via fs.service. Returns fd or negative errno.
pub fn open_via_fs(path: &str) -> Result<u64, i64> {
    let fs_tid = find_fs_service().ok_or(-3)?;
    let mut path_buf = [0u8; FS_PATH_MAX];
    let bytes = path.as_bytes();
    if bytes.len() >= FS_PATH_MAX {
        return Err(-22);
    }
    path_buf[..bytes.len()].copy_from_slice(bytes);
    let req = FsRequestIp { op: FsRequestIp::OP_OPEN, arg1: 0, arg2: 0, path: path_buf };
    let resp = fs_ipc_request(fs_tid, &req).map_err(|_| -5)?;
    if resp.status < 0 {
        return Err(resp.status);
    }
    Ok(resp.status as u64)
}

/// Read via fs.service into out buffer. Returns bytes read or negative errno.
pub fn read_via_fs(fd: u64, out: &mut [u8]) -> Result<usize, i64> {
    let fs_tid = find_fs_service().ok_or(-3)?;
    let req = FsRequestIp { op: FsRequestIp::OP_READ, arg1: fd, arg2: out.len() as u64, path: [0u8; FS_PATH_MAX] };
    let resp = fs_ipc_request(fs_tid, &req).map_err(|_| -5)?;
    if resp.status < 0 {
        return Err(resp.status);
    }
    let n = resp.len as usize;
    if n > out.len() || n > FS_DATA_MAX {
        return Err(-5);
    }
    out[..n].copy_from_slice(&resp.data[..n]);
    Ok(n)
}

/// Close via fs.service (best effort)
pub fn close_via_fs(fd: u64) {
    if let Some(fs_tid) = find_fs_service() {
        let req = FsRequestIp { op: FsRequestIp::OP_CLOSE, arg1: fd, arg2: 0, path: [0u8; FS_PATH_MAX] };
        let _ = fs_ipc_request(fs_tid, &req);
    }
}

/// Convenience: read whole file via fs.service (or None on error)
pub fn read_file_via_fs(path: &str, max_size: usize) -> Option<Vec<u8>> {
    let fd = open_via_fs(path).ok()?;
    let mut out = Vec::new();
    let mut chunk = [0u8; FS_DATA_MAX];
    while out.len() < max_size {
        let to_read = core::cmp::min(chunk.len(), max_size - out.len());
        match read_via_fs(fd, &mut chunk[..to_read]) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(_) => { close_via_fs(fd); return None; }
        }
    }
    close_via_fs(fd);
    if out.is_empty() { None } else { Some(out) }
}
