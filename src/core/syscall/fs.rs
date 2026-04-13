//! ファイルシステム関連のシステムコール

use super::types::{EBADF, EFAULT, EINVAL, EIO, ENOENT, ENOSYS, ENOTDIR, ESRCH, SUCCESS};
use crate::task::fd_table::{FdTable, FileHandle, FD_BASE, O_CLOEXEC, PROCESS_MAX_FDS};
use alloc::string::String;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::AtomicUsize;

// グローバル FD テーブルは廃止。各プロセスの Process::fd_table を使用する。

#[inline]
fn current_process_id_raw() -> Option<u64> {
    crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id().as_u64()))
}

/// 現在プロセスの FD テーブルを読み取り専用で操作する。
fn with_fd_table<F, R>(pid_raw: u64, f: F) -> Option<R>
where
    F: FnOnce(&FdTable) -> R,
{
    let pid = crate::task::ids::ProcessId::from_u64(pid_raw);
    crate::task::with_process(pid, |p| f(p.fd_table()))
}

/// 現在プロセスの FD テーブルを可変で操作する。
fn with_fd_table_mut<F, R>(pid_raw: u64, f: F) -> Option<R>
where
    F: FnOnce(&mut FdTable) -> R,
{
    let pid = crate::task::ids::ProcessId::from_u64(pid_raw);
    crate::task::with_process_mut(pid, |p| f(p.fd_table_mut()))
}

// ファイルシステムIPC定数（swiftlib::fs_constsと同一の値を維持）
const FS_PATH_MAX: usize = 128;
const FS_DATA_MAX: usize = 4096;
const IPC_MAX_MSG_SIZE: usize = 65536;
const FS_RECV_TIMEOUT_TICKS: u64 = 500;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct FsRequest {
    pub(crate) op: u64,
    pub(crate) arg1: u64,
    pub(crate) arg2: u64,
    pub(crate) path: [u8; FS_PATH_MAX],
}

impl FsRequest {
    pub(crate) const OP_OPEN: u64 = 1;
    pub(crate) const OP_READ: u64 = 2;
    pub(crate) const OP_CLOSE: u64 = 4;
    pub(crate) const OP_STAT: u64 = 6;
    pub(crate) const OP_FSTAT: u64 = 7;
    pub(crate) const OP_READDIR: u64 = 8;
    pub(crate) const OP_EXEC_STREAM: u64 = 9;
    pub(crate) const OP_READDIR_ALL: u64 = 10;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct FsResponse {
    pub(crate) status: i64,
    pub(crate) len: u64,
    pub(crate) data: [u8; FS_DATA_MAX],
}

static CACHED_FS_SERVICE_TID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[inline]
pub(crate) fn fs_service_tid() -> Option<u64> {
    let _ = &CACHED_FS_SERVICE_TID;
    None
}

fn recv_from_fs_with_timeout(fs_tid: u64, buf: &mut [u8]) -> Result<usize, u64> {
    let start_tick = crate::syscall::time::get_ticks();
    loop {
        if !crate::task::thread_id_exists(fs_tid) {
            return Err(EIO);
        }

        if let Some(n) = crate::syscall::ipc::recv_from_sender_for_kernel_nonblocking(fs_tid, buf)?
        {
            return Ok(n);
        }

        if crate::syscall::time::get_ticks().saturating_sub(start_tick) > FS_RECV_TIMEOUT_TICKS {
            return Err(EIO);
        }
        crate::task::yield_now();
    }
}

pub(crate) fn fs_service_request(fs_tid: u64, req: &FsRequest) -> Result<FsResponse, u64> {
    let req_slice = unsafe {
        core::slice::from_raw_parts(
            req as *const _ as *const u8,
            core::mem::size_of::<FsRequest>(),
        )
    };
    if crate::syscall::ipc::send_from_kernel(fs_tid, req_slice) {
        let mut resp_buf = [0u8; core::mem::size_of::<FsResponse>()];
        let n = recv_from_fs_with_timeout(fs_tid, &mut resp_buf)?;
        if n < core::mem::size_of::<FsResponse>() {
            return Err(EIO);
        }
        let resp: FsResponse =
            unsafe { core::ptr::read_unaligned(resp_buf.as_ptr() as *const FsResponse) };
        Ok(resp)
    } else {
        Err(EIO)
    }
}

// Internal: send request then receive a streamed image from stream backend. Protocol:
// 1) send FsRequest with OP_EXEC_STREAM
// 2) receive initial FsResponse (status,len) where len == total image size
// 3) receive raw data chunks (no per-chunk header) until total bytes received == initial len
fn fs_service_request_stream(fs_tid: u64, req: &FsRequest) -> Result<Vec<u8>, u64> {
    let req_slice = unsafe {
        core::slice::from_raw_parts(
            req as *const _ as *const u8,
            core::mem::size_of::<FsRequest>(),
        )
    };
    if !crate::syscall::ipc::send_from_kernel(fs_tid, req_slice) {
        return Err(EIO);
    }
    // receive initial FsResponse header
    let mut header_buf = [0u8; core::mem::size_of::<FsResponse>()];
    let n = recv_from_fs_with_timeout(fs_tid, &mut header_buf)?;
    if n < core::mem::size_of::<FsResponse>() {
        return Err(EIO);
    }
    let header: FsResponse =
        unsafe { core::ptr::read_unaligned(header_buf.as_ptr() as *const FsResponse) };
    if header.status < 0 {
        return Err((-header.status) as u64);
    }
    let total = header.len as usize;
    if total == 0 {
        return Ok(Vec::new());
    }
    if total > 8 * 1024 * 1024 {
        return Err(EINVAL);
    }
    // allocate destination buffer once; receive directly into it to avoid intermediate copies
    let mut out = vec![0u8; total];
    let mut received = 0usize;
    while received < total {
        let remaining = total - received;
        let recv_len = core::cmp::min(remaining, IPC_MAX_MSG_SIZE);
        let dst = &mut out[received..received + recv_len];
        let n = recv_from_fs_with_timeout(fs_tid, dst)?;
        if n == 0 {
            return Err(EIO);
        }
        received += n;
    }
    if received != total {
        return Err(EIO);
    }
    Ok(out)
}

pub(crate) fn exec_image_via_fs(path: &str) -> Result<Vec<u8>, u64> {
    let fs_tid = fs_service_tid().ok_or(ESRCH)?;
    let req = FsRequest {
        op: FsRequest::OP_EXEC_STREAM,
        arg1: 0,
        arg2: 0,
        path: encode_fs_path(path)?,
    };
    fs_service_request_stream(fs_tid, &req)
}

pub(crate) fn encode_fs_path(path: &str) -> Result<[u8; FS_PATH_MAX], u64> {
    let mut out = [0u8; FS_PATH_MAX];
    let bytes = path.as_bytes();
    if bytes.is_empty() || bytes.len() >= FS_PATH_MAX {
        return Err(EINVAL);
    }
    if bytes.iter().any(|&b| b == 0) {
        return Err(EINVAL);
    }
    out[..bytes.len()].copy_from_slice(bytes);
    Ok(out)
}

fn open_via_fs_service(path: &str, flags: u64) -> Result<u64, u64> {
    let fs_tid = fs_service_tid().ok_or(ESRCH)?;
    let req = FsRequest {
        op: FsRequest::OP_OPEN,
        arg1: 0,
        arg2: flags,
        path: encode_fs_path(path)?,
    };
    let resp = fs_service_request(fs_tid, &req)?;
    if resp.status < 0 {
        return Err((-resp.status) as u64);
    }
    Ok(resp.status as u64)
}

fn read_via_fs_service(fd_remote: u64, out: &mut [u8]) -> Result<usize, u64> {
    let fs_tid = fs_service_tid().ok_or(ESRCH)?;
    let req = FsRequest {
        op: FsRequest::OP_READ,
        arg1: fd_remote,
        arg2: out.len() as u64,
        path: [0; FS_PATH_MAX],
    };
    let resp = fs_service_request(fs_tid, &req)?;
    if resp.status < 0 {
        return Err((-resp.status) as u64);
    }
    let n = core::cmp::min(
        resp.len as usize,
        core::cmp::min(out.len(), resp.data.len()),
    );
    out[..n].copy_from_slice(&resp.data[..n]);
    Ok(n)
}

fn close_via_fs_service(fd_remote: u64) -> u64 {
    let fs_tid = match fs_service_tid() {
        Some(t) => t,
        None => return ESRCH,
    };
    let req = FsRequest {
        op: FsRequest::OP_CLOSE,
        arg1: fd_remote,
        arg2: 0,
        path: [0; FS_PATH_MAX],
    };
    match fs_service_request(fs_tid, &req) {
        Ok(resp) => {
            if resp.status < 0 {
                (-resp.status) as u64
            } else {
                SUCCESS
            }
        }
        Err(e) => e,
    }
}

pub(crate) fn close_remote_fd_from_kernel(fd_remote: u64) {
    let _ = close_via_fs_service(fd_remote);
}

fn stat_path_via_fs_service(path: &str) -> Result<(u16, u64), u64> {
    let fs_tid = fs_service_tid().ok_or(ESRCH)?;
    let req = FsRequest {
        op: FsRequest::OP_STAT,
        arg1: 0,
        arg2: 0,
        path: encode_fs_path(path)?,
    };
    let resp = fs_service_request(fs_tid, &req)?;
    if resp.status < 0 {
        return Err((-resp.status) as u64);
    }
    Ok((resp.status as u16, resp.len))
}

fn fstat_via_fs_service(fd_remote: u64) -> Result<(u16, u64), u64> {
    let fs_tid = fs_service_tid().ok_or(ESRCH)?;
    let req = FsRequest {
        op: FsRequest::OP_FSTAT,
        arg1: fd_remote,
        arg2: 0,
        path: [0; FS_PATH_MAX],
    };
    let resp = fs_service_request(fs_tid, &req)?;
    if resp.status < 0 {
        return Err((-resp.status) as u64);
    }
    Ok((resp.status as u16, resp.len))
}

fn readdir_chunk_via_fs_service(
    fd_remote: u64,
    start_index: usize,
    out: &mut [u8],
) -> Result<(usize, usize), u64> {
    let fs_tid = fs_service_tid().ok_or(ESRCH)?;
    let max_bytes = out.len().min(FS_DATA_MAX).min(u32::MAX as usize);
    let start = start_index.min(u32::MAX as usize);
    let req = FsRequest {
        op: FsRequest::OP_READDIR,
        arg1: fd_remote,
        arg2: ((start as u64) << 32) | (max_bytes as u64),
        path: [0; FS_PATH_MAX],
    };
    let resp = fs_service_request(fs_tid, &req)?;
    if resp.status < 0 {
        return Err((-resp.status) as u64);
    }
    let next_index = usize::try_from(resp.status).map_err(|_| EIO)?;
    let n = core::cmp::min(
        resp.len as usize,
        core::cmp::min(out.len(), resp.data.len()),
    );
    out[..n].copy_from_slice(&resp.data[..n]);
    Ok((n, next_index))
}

fn read_cstring(ptr: u64) -> Result<String, u64> {
    crate::syscall::read_user_cstring(ptr, 1024)
}

#[inline]
fn mode_is_directory(mode: u16) -> bool {
    (mode & 0xF000) == 0x4000
}

#[inline]
fn mode_for_stat(mode: u16) -> u32 {
    let mut out = mode as u32;
    if (out & 0xF000) == 0 {
        out |= 0x8000;
    }
    if (out & 0o777) == 0 {
        out |= 0o755;
    }
    out
}

#[inline]
fn should_fallback_to_initfs(errno: u64) -> bool {
    // Prefer ATA rootfs and fallback to initfs only when rootfs path is unavailable.
    errno == ESRCH || errno == ENOENT || errno == ENOTDIR || errno == EBADF
}

#[inline]
fn fallback_file_metadata(path: &str) -> Option<(u16, u64)> {
    if crate::kmod::fs::is_mounted() {
        crate::kmod::fs::file_metadata(path)
    } else {
        crate::kmod::fs::file_metadata(path).or_else(|| crate::init::fs::file_metadata(path))
    }
}

#[inline]
fn fallback_is_directory(path: &str) -> bool {
    if crate::kmod::fs::is_mounted() {
        crate::kmod::fs::is_directory(path)
    } else {
        crate::kmod::fs::is_directory(path) || crate::init::fs::is_directory(path)
    }
}

#[inline]
fn fallback_readdir(path: &str) -> Option<Vec<String>> {
    if crate::kmod::fs::is_mounted() {
        crate::kmod::fs::readdir_path(path)
    } else {
        crate::kmod::fs::readdir_path(path).or_else(|| crate::init::fs::readdir_path(path))
    }
}

fn parse_readdir_names(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for raw in bytes.split(|&b| b == b'\n') {
        if raw.is_empty() {
            continue;
        }
        if let Ok(name) = core::str::from_utf8(raw) {
            if !name.is_empty() {
                out.push(name.to_string());
            }
        }
    }
    out
}

fn parse_readdir_typed(bytes: &[u8]) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    for record in bytes.split(|&b| b == b'\n') {
        if record.len() < 2 {
            continue;
        }
        let dtype = record[record.len() - 1];
        if !(1..=8).contains(&dtype) {
            continue;
        }
        if record.len() >= 2 && record[record.len() - 2] == 0 {
            let name_bytes = &record[..record.len() - 2];
            if let Ok(name) = core::str::from_utf8(name_bytes) {
                if !name.is_empty() {
                    out.push((name.to_string(), dtype));
                }
            }
        }
    }
    out
}

/// パスを正規化する（`.` / `..` を解決し重複スラッシュを除去）
fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        alloc::format!("/{}", parts.join("/"))
    }
}

/// プロセスの CWD を基に相対パスを絶対パスへ解決する
fn resolve_path(pid_raw: u64, path: &str) -> String {
    if path.starts_with('/') {
        normalize_path(path)
    } else {
        let pid = crate::task::ids::ProcessId::from_u64(pid_raw);
        let cwd = crate::task::with_process(pid, |p| {
            let mut s = alloc::string::String::new();
            s.push_str(p.cwd());
            s
        })
        .unwrap_or_else(|| "/".to_string());
        normalize_path(&alloc::format!("{}/{}", cwd.trim_end_matches('/'), path))
    }
}

const FS_SERVICE_RETRY_COUNT: usize = 3;
const FS_SERVICE_RETRY_MS: u64 = 10;

fn make_tty_handle() -> alloc::boxed::Box<FileHandle> {
    alloc::boxed::Box::new(FileHandle {
        data: alloc::boxed::Box::new([]),
        pos: 0,
        dir_path: Some("/dev/tty".to_string()),
        is_remote: false,
        fd_remote: 0,
        remote_refs: None,
        pipe_id: None,
        pipe_write: false,
    })
}

fn open_resolved_for_pid(owner_pid: u64, path: &str, flags: u64) -> u64 {
    if path == "/dev/tty" || path == "/dev/stdin" || path == "/dev/stdout" || path == "/dev/stderr"
    {
        let cloexec = (flags & O_CLOEXEC) != 0;
        return match with_fd_table_mut(owner_pid, |t| t.alloc(make_tty_handle(), cloexec)) {
            Some(Some(fd)) => fd as u64,
            _ => ENOSYS,
        };
    }

    // カーネル内で管理する FD_CLOEXEC は fs.service へ渡さない。
    let backend_flags = flags & !O_CLOEXEC;
    let mut last_err = 0u64;
    let mut opened = None;
    for _ in 0..FS_SERVICE_RETRY_COUNT {
        match open_via_fs_service(path, backend_flags) {
            Ok(remote_fd) => {
                opened = Some(remote_fd);
                break;
            }
            Err(e) => {
                last_err = e;
                if e == EIO {
                    crate::task::yield_now();
                    continue;
                } else {
                    break;
                }
            }
        }
    }

    let (data_vec, dir_path, is_remote, fd_remote) = match opened {
        Some(remote_fd) => {
            // fstatをスキップ: ディレクトリ判定はreaddir時に遅延
            (Vec::new(), None, true, remote_fd)
        }
        None => {
            let errno = if last_err != 0 { last_err } else { EIO };
            if should_fallback_to_initfs(errno) {
                if fallback_is_directory(path) {
                    (Vec::new(), Some(path.to_string()), false, 0)
                } else {
                    match crate::kmod::fs::read_all(path) {
                        Some(d) => (d, None, false, 0),
                        None => return ENOENT,
                    }
                }
            } else {
                return errno;
            }
        }
    };

    let cloexec = (flags & O_CLOEXEC) != 0;
    let handle = alloc::boxed::Box::new(FileHandle {
        data: data_vec.into_boxed_slice(),
        pos: 0,
        dir_path,
        is_remote,
        fd_remote,
        remote_refs: if is_remote {
            Some(Arc::new(AtomicUsize::new(1)))
        } else {
            None
        },
        pipe_id: None,
        pipe_write: false,
    });

    match with_fd_table_mut(owner_pid, |t| t.alloc(handle, cloexec)) {
        Some(Some(fd)) => fd as u64,
        _ => {
            if is_remote {
                let _ = close_via_fs_service(fd_remote);
            }
            ENOSYS
        }
    }
}

/// Openシステムコール (initfs の読み取り専用をサポートする簡易実装)
pub fn open(path_ptr: u64, flags: u64) -> u64 {
    let owner_pid = match current_process_id_raw() {
        Some(pid) => pid,
        None => return EBADF,
    };

    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let path = resolve_path(owner_pid, &path);
    open_resolved_for_pid(owner_pid, &path, flags)
}

/// Closeシステムコール
pub fn close(fd: u64) -> u64 {
    if fd < FD_BASE as u64 {
        return EBADF;
    }
    let idx = fd as usize;
    if idx >= PROCESS_MAX_FDS {
        return EBADF;
    }
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };
    let handle = with_fd_table_mut(pid, |t| t.take(idx));
    match handle {
        Some(Some(_h)) => SUCCESS,
        _ => EBADF,
    }
}

/// Seekシステムコール
pub fn seek(fd: u64, offset: i64, whence: u64) -> u64 {
    if fd < FD_BASE as u64 {
        return ENOSYS;
    }
    let idx = fd as usize;
    if idx >= PROCESS_MAX_FDS {
        return EBADF;
    }
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };

    match with_fd_table_mut(pid, |t| {
        let fh = t.get_mut(idx).ok_or(EBADF)?;
        let remote_len = if fh.is_remote {
            let (_, size) = fstat_via_fs_service(fh.fd_remote)?;
            Some(i64::try_from(size).map_err(|_| EINVAL)?)
        } else {
            None
        };
        let new_pos = match whence {
            0 => offset,
            1 => fh.pos as i64 + offset,
            2 => {
                let len = remote_len.unwrap_or(fh.data.len() as i64);
                len + offset
            }
            _ => return Err(EINVAL),
        };
        if new_pos < 0 {
            return Err(EINVAL);
        }
        let new_pos = if fh.is_remote {
            new_pos as usize
        } else {
            core::cmp::min(new_pos as usize, fh.data.len())
        };
        fh.pos = new_pos;
        Ok(fh.pos as u64)
    }) {
        Some(Ok(pos)) => pos,
        Some(Err(e)) => e,
        None => EBADF,
    }
}

/// Linux x86_64 struct stat をユーザーバッファに書き込む
///
/// struct stat のレイアウト (144 バイト):
///   0:  st_dev    (u64)
///   8:  st_ino    (u64)
///   16: st_nlink  (u64)
///   24: st_mode   (u32)
///   28: st_uid    (u32)
///   32: st_gid    (u32)
///   36: __pad0    (u32)
///   40: st_rdev   (u64)
///   48: st_size   (i64)
///   56: st_blksize (i64)
///   64: st_blocks  (i64)  — 512 バイト単位
///   72-143: timespec × 3 + unused (ゼロ)
fn write_stat_buf(stat_ptr: u64, mode: u32, size: u64) {
    const STAT_SIZE: usize = 144;
    let blocks = size.div_ceil(512);
    crate::syscall::with_user_memory_access(|| unsafe {
        let buf = core::slice::from_raw_parts_mut(stat_ptr as *mut u8, STAT_SIZE);
        buf.fill(0);
        // st_dev = 1 (仮のデバイス番号)
        buf[0..8].copy_from_slice(&1u64.to_ne_bytes());
        // st_ino = 1 (inode 番号は省略)
        buf[8..16].copy_from_slice(&1u64.to_ne_bytes());
        // st_nlink = 1
        buf[16..24].copy_from_slice(&1u64.to_ne_bytes());
        // st_mode
        buf[24..28].copy_from_slice(&mode.to_ne_bytes());
        // st_size
        buf[48..56].copy_from_slice(&size.to_ne_bytes());
        // st_blksize = 4096
        buf[56..64].copy_from_slice(&4096u64.to_ne_bytes());
        // st_blocks
        buf[64..72].copy_from_slice(&blocks.to_ne_bytes());
    });
}

/// Fstatシステムコール
pub fn fstat(fd: u64, stat_ptr: u64) -> u64 {
    if stat_ptr == 0 {
        return EFAULT;
    }
    const STAT_SIZE: u64 = 144;
    if !crate::syscall::validate_user_ptr(stat_ptr, STAT_SIZE) {
        return EFAULT;
    }

    if fd < FD_BASE as u64 {
        // stdin/stdout/stderr → キャラクタデバイス (S_IFCHR | 0666 = 0x2000 | 0o666)
        write_stat_buf(stat_ptr, 0x2000 | 0o666, 0);
        return SUCCESS;
    }

    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };
    let idx = fd as usize;
    if idx >= PROCESS_MAX_FDS {
        return EBADF;
    }

    // FileHandle からメタデータを取得する
    let file_info = with_fd_table(pid, |t| {
        t.get(idx).map(|fh| {
            (
                fh.data.len() as u64,
                fh.dir_path.is_some(),
                fh.is_remote,
                fh.fd_remote,
            )
        })
    });
    let (size, is_dir, is_remote, fd_remote) = match file_info {
        Some(Some(v)) => v,
        _ => return EBADF,
    };
    if is_remote {
        let (mode, size) = match fstat_via_fs_service(fd_remote) {
            Ok(v) => v,
            Err(e) => return e,
        };
        write_stat_buf(stat_ptr, mode_for_stat(mode), size);
        return SUCCESS;
    }
    // S_IFREG = 0x8000, S_IFDIR = 0x4000
    let mode = if is_dir {
        0x4000u32 | 0o755
    } else {
        0x8000u32 | 0o755
    };
    write_stat_buf(stat_ptr, mode, size);
    SUCCESS
}

/// Statシステムコール
pub fn stat(path_ptr: u64, stat_ptr: u64) -> u64 {
    if path_ptr == 0 || stat_ptr == 0 {
        return EINVAL;
    }
    const STAT_SIZE: u64 = 144;
    if !crate::syscall::validate_user_ptr(stat_ptr, STAT_SIZE) {
        return EFAULT;
    }
    let owner_pid = match current_process_id_raw() {
        Some(pid) => pid,
        None => return EBADF,
    };
    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let resolved = resolve_path(owner_pid, &path);

    match stat_path_via_fs_service(&resolved) {
        Ok((mode, size)) => {
            write_stat_buf(stat_ptr, mode_for_stat(mode), size);
            SUCCESS
        }
        Err(errno) if should_fallback_to_initfs(errno) => {
            match fallback_file_metadata(&resolved) {
                Some((inode_mode, size)) => {
                    write_stat_buf(stat_ptr, mode_for_stat(inode_mode), size);
                    SUCCESS
                }
                None => ENOENT,
            }
        }
        Err(errno) => errno,
    }
}

/// Mkdirシステムコール（読み取り専用ファイルシステムのため未実装）
pub fn mkdir(_path_ptr: u64, _mode: u64) -> u64 {
    ENOSYS
}

/// Rmdirシステムコール（読み取り専用ファイルシステムのため未実装）
pub fn rmdir(_path_ptr: u64) -> u64 {
    ENOSYS
}

/// Readdirシステムコール
pub fn readdir(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_ptr == 0 || buf_len == 0 {
        return EINVAL;
    }
    if !crate::syscall::validate_user_ptr(buf_ptr, buf_len) {
        return EFAULT;
    }
    if fd < FD_BASE as u64 {
        return EBADF;
    }
    let idx = fd as usize;
    if idx >= PROCESS_MAX_FDS {
        return EBADF;
    }
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };

    let (dir_path, start_pos, is_remote, fd_remote) = match with_fd_table(pid, |t| {
        t.get(idx)
            .map(|fh| (fh.dir_path.clone(), fh.pos, fh.is_remote, fh.fd_remote))
    }) {
        Some(Some((Some(p), pos, is_remote, fd_remote))) => (p, pos, is_remote, fd_remote),
        _ => return EBADF,
    };

    if is_remote {
        let mut offset = start_pos;
        let mut copied = 0usize;
        let mut chunk = [0u8; FS_DATA_MAX];
        while copied < buf_len as usize {
            let want = core::cmp::min(chunk.len(), buf_len as usize - copied);
            let (n, next_index) =
                match readdir_chunk_via_fs_service(fd_remote, offset, &mut chunk[..want]) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
            if n == 0 {
                break;
            }
            crate::syscall::with_user_memory_access(|| unsafe {
                let dst = core::slice::from_raw_parts_mut((buf_ptr + copied as u64) as *mut u8, n);
                dst.copy_from_slice(&chunk[..n]);
            });
            copied += n;
            offset = next_index;
            if n < want {
                break;
            }
        }
        let _ = with_fd_table_mut(pid, |t| {
            if let Some(fh) = t.get_mut(idx) {
                fh.pos = offset;
            }
        });
        return copied as u64;
    }

    let names = match fallback_readdir(&dir_path) {
        Some(n) => n,
        None => return EINVAL,
    };
    let joined = names.join("\n");
    let bytes = joined.as_bytes();
    let to_copy = core::cmp::min(bytes.len(), buf_len as usize);
    crate::syscall::with_user_memory_access(|| unsafe {
        let dst = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, to_copy);
        dst.copy_from_slice(&bytes[..to_copy]);
    });
    to_copy as u64
}

/// Chdirシステムコール
pub fn chdir(path_ptr: u64) -> u64 {
    if path_ptr == 0 {
        return EINVAL;
    }
    let pid_raw = match current_process_id_raw() {
        Some(pid) => pid,
        None => return EBADF,
    };
    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let resolved = resolve_path(pid_raw, &path);
    match stat_path_via_fs_service(&resolved) {
        Ok((mode, _)) => {
            if !mode_is_directory(mode) {
                return ENOTDIR;
            }
        }
        Err(errno) if should_fallback_to_initfs(errno) => {
            if !fallback_is_directory(&resolved) {
                return ENOTDIR;
            }
        }
        Err(errno) => return errno,
    }
    let pid = crate::task::ids::ProcessId::from_u64(pid_raw);
    crate::task::with_process_mut(pid, |p| p.set_cwd(&resolved));
    SUCCESS
}

/// Getcwdシステムコール
pub fn getcwd(buf_ptr: u64, size: u64) -> u64 {
    if buf_ptr == 0 || size == 0 {
        return EINVAL;
    }
    if !crate::syscall::validate_user_ptr(buf_ptr, size) {
        return EFAULT;
    }
    let pid_raw = match current_process_id_raw() {
        Some(pid) => pid,
        None => return EFAULT,
    };
    let pid = crate::task::ids::ProcessId::from_u64(pid_raw);
    let mut tmp = [0u8; 256];
    let cwd_len = crate::task::with_process(pid, |p| {
        let s = p.cwd().as_bytes();
        let n = s.len().min(255);
        tmp[..n].copy_from_slice(&s[..n]);
        n
    })
    .unwrap_or(1);
    let needed = cwd_len + 1;
    if (size as usize) < needed {
        return EINVAL;
    }
    crate::syscall::with_user_memory_access(|| unsafe {
        core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf_ptr as *mut u8, cwd_len);
        *(buf_ptr as *mut u8).add(cwd_len) = 0;
    });
    buf_ptr
}

/// Read: 開かれたファイルからデータを読み込む
pub fn read(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if buf_ptr == 0 {
        return EFAULT;
    }
    if len == 0 {
        return 0;
    }
    if !crate::syscall::validate_user_ptr(buf_ptr, len) {
        return EFAULT;
    }
    if fd < FD_BASE as u64 {
        return EBADF;
    }
    let idx = fd as usize;
    if idx >= PROCESS_MAX_FDS {
        return EBADF;
    }
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };

    let (is_remote, fd_remote) =
        match with_fd_table(pid, |t| t.get(idx).map(|fh| (fh.is_remote, fh.fd_remote))) {
            Some(Some(v)) => v,
            _ => return EBADF,
        };

    if is_remote {
        let mut tmp = alloc::vec![0u8; len as usize];
        let n = match read_via_fs_service(fd_remote, &mut tmp) {
            Ok(v) => v,
            Err(e) => return e,
        };
        if n > 0 {
            crate::syscall::with_user_memory_access(|| unsafe {
                let dst = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, n);
                dst.copy_from_slice(&tmp[..n]);
            });
        }
        return n as u64;
    }

    let local = match with_fd_table_mut(pid, |t| {
        let fh = t.get_mut(idx)?;
        let avail = fh.data.len().saturating_sub(fh.pos);
        if avail == 0 {
            return Some(Vec::new());
        }
        let to_read = core::cmp::min(avail, len as usize);
        let mut data = Vec::with_capacity(to_read);
        data.extend_from_slice(&fh.data[fh.pos..fh.pos + to_read]);
        fh.pos += to_read;
        Some(data)
    }) {
        Some(Some(v)) => v,
        _ => return EBADF,
    };

    if local.is_empty() {
        return 0;
    }

    crate::syscall::with_user_memory_access(|| unsafe {
        let dst = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, local.len());
        dst.copy_from_slice(&local);
    });
    local.len() as u64
}

/// Fcntl システムコール（FD フラグ操作）
///
/// - F_GETFD (1): FD フラグを取得
/// - F_SETFD (2): FD フラグを設定
/// - F_GETFL (3): ファイル状態フラグを取得（スタブ: 0 を返す）
/// - F_SETFL (4): ファイル状態フラグを設定（スタブ: 成功を返す）
pub fn fcntl(fd: u64, cmd: u64, arg: u64) -> u64 {
    use crate::task::fd_table::FD_CLOEXEC;
    const F_GETFD: u64 = 1;
    const F_SETFD: u64 = 2;
    const F_GETFL: u64 = 3;
    const F_SETFL: u64 = 4;

    if fd < FD_BASE as u64 {
        // stdin/stdout/stderr: FD フラグは 0
        return match cmd {
            F_GETFD | F_GETFL => 0,
            F_SETFD | F_SETFL => SUCCESS,
            _ => EINVAL,
        };
    }
    let idx = fd as usize;
    if idx >= PROCESS_MAX_FDS {
        return EBADF;
    }
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };

    match cmd {
        F_GETFD => match with_fd_table(pid, |t| t.get_flags(idx)) {
            Some(Some(flags)) => flags as u64,
            _ => EBADF,
        },
        F_SETFD => {
            let cloexec = (arg & 1) != 0;
            let new_flags = if cloexec { FD_CLOEXEC } else { 0 };
            match with_fd_table_mut(pid, |t| t.set_flags(idx, new_flags)) {
                Some(true) => SUCCESS,
                _ => EBADF,
            }
        }
        F_GETFL => 0, // O_RDONLY スタブ
        F_SETFL => SUCCESS,
        _ => EINVAL,
    }
}

/// Dup システムコール: FD を複製して最小の空き番号に割り当てる
pub fn dup(fd: u64) -> u64 {
    if fd < FD_BASE as u64 {
        let pid = match current_process_id_raw() {
            Some(p) => p,
            None => return EBADF,
        };
        return match with_fd_table_mut(pid, |t| t.alloc(make_tty_handle(), false)) {
            Some(Some(new_fd)) => new_fd as u64,
            _ => ENOSYS,
        };
    }
    let idx = fd as usize;
    if idx >= PROCESS_MAX_FDS {
        return EBADF;
    }
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };

    // 既存エントリをクローンして新しい FD を割り当てる
    let cloned = with_fd_table(pid, |t| {
        t.get(idx).map(|fh| {
            alloc::boxed::Box::new(FileHandle {
                data: fh.data.clone(),
                pos: fh.pos,
                dir_path: fh.dir_path.clone(),
                is_remote: fh.is_remote,
                fd_remote: fh.fd_remote,
                remote_refs: fh.clone_remote_refs(),
                pipe_id: fh.pipe_id,
                pipe_write: fh.pipe_write,
            })
        })
    });
    let new_handle = match cloned {
        Some(Some(h)) => h,
        _ => return EBADF,
    };

    match with_fd_table_mut(pid, |t| t.alloc(new_handle, false)) {
        Some(Some(new_fd)) => new_fd as u64,
        _ => ENOSYS,
    }
}

/// Dup2 システムコール: FD を指定した番号に複製する
pub fn dup2(old_fd: u64, new_fd: u64) -> u64 {
    if new_fd < FD_BASE as u64 || new_fd as usize >= PROCESS_MAX_FDS {
        return EBADF;
    }
    if old_fd == new_fd {
        // old_fd が有効かどうかだけ確認
        if old_fd < FD_BASE as u64 {
            return old_fd;
        }
        let pid = match current_process_id_raw() {
            Some(p) => p,
            None => return EBADF,
        };
        return match with_fd_table(pid, |t| t.get(old_fd as usize).is_some()) {
            Some(true) => old_fd,
            _ => EBADF,
        };
    }

    let new_idx = new_fd as usize;
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };

    let new_handle = if old_fd < FD_BASE as u64 {
        make_tty_handle()
    } else {
        let old_idx = old_fd as usize;
        if old_idx >= PROCESS_MAX_FDS {
            return EBADF;
        }
        let cloned = with_fd_table(pid, |t| {
            t.get(old_idx).map(|fh| {
                alloc::boxed::Box::new(FileHandle {
                    data: fh.data.clone(),
                    pos: fh.pos,
                    dir_path: fh.dir_path.clone(),
                    is_remote: fh.is_remote,
                    fd_remote: fh.fd_remote,
                    remote_refs: fh.clone_remote_refs(),
                    pipe_id: fh.pipe_id,
                    pipe_write: fh.pipe_write,
                })
            })
        });
        match cloned {
            Some(Some(h)) => h,
            _ => return EBADF,
        }
    };

    // new_fd が使用中なら閉じる
    with_fd_table_mut(pid, |t| {
        t.close_fd(new_idx);
        let ptr = alloc::boxed::Box::into_raw(new_handle) as u64;
        t.entries[new_idx] = ptr;
        t.flags[new_idx] = 0;
    });

    new_fd
}

/// Openat システムコール
///
/// AT_FDCWD(-100) の場合は CWD 相対の open() と同等。
/// それ以外の dirfd は fd_table からディレクトリパスを取得してプレフィックスとして使用する。
pub fn openat(dirfd: i64, path_ptr: u64, flags: u64, _mode: u64) -> u64 {
    const AT_FDCWD: i64 = -100;

    if dirfd == AT_FDCWD {
        // CWD 相対 → 通常の open() と同じ
        return open(path_ptr, flags);
    }

    // dirfd が示すディレクトリを取得
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };
    let idx = dirfd as usize;
    if idx >= PROCESS_MAX_FDS {
        return EBADF;
    }
    let dir_path = match with_fd_table(pid, |t| t.get(idx).and_then(|fh| fh.dir_path.clone())) {
        Some(Some(p)) => p,
        _ => return EBADF,
    };

    // path を dir_path に対して解決する
    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let full_path = if path.starts_with('/') {
        path
    } else {
        alloc::format!("{}/{}", dir_path.trim_end_matches('/'), path)
    };

    open_resolved_for_pid(pid, &normalize_path(&full_path), flags)
}

/// Newfstatat (fstatat) システムコール
///
/// AT_FDCWD(-100) の場合は stat() と同等。
pub fn newfstatat(dirfd: i64, path_ptr: u64, stat_ptr: u64, flags: u64) -> u64 {
    const AT_FDCWD: i64 = -100;
    const AT_EMPTY_PATH: u64 = 0x1000;

    // AT_EMPTY_PATH: path が空の場合は dirfd 自体を fstat する
    if (flags & AT_EMPTY_PATH) != 0 {
        if dirfd == AT_FDCWD {
            return stat(path_ptr, stat_ptr);
        }
        return fstat(dirfd as u64, stat_ptr);
    }

    if dirfd == AT_FDCWD {
        return stat(path_ptr, stat_ptr);
    }

    // dirfd 相対パスを解決して stat
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };
    let idx = dirfd as usize;
    if idx >= PROCESS_MAX_FDS {
        return EBADF;
    }
    let dir_path = match with_fd_table(pid, |t| t.get(idx).and_then(|fh| fh.dir_path.clone())) {
        Some(Some(p)) => p,
        _ => return EBADF,
    };
    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let full = if path.starts_with('/') {
        normalize_path(&path)
    } else {
        normalize_path(&alloc::format!(
            "{}/{}",
            dir_path.trim_end_matches('/'),
            path
        ))
    };
    match stat_path_via_fs_service(&full) {
        Ok((mode, size)) => {
            const STAT_SIZE: u64 = 144;
            if !crate::syscall::validate_user_ptr(stat_ptr, STAT_SIZE) {
                return EFAULT;
            }
            write_stat_buf(stat_ptr, mode_for_stat(mode), size);
            SUCCESS
        }
        Err(errno) if should_fallback_to_initfs(errno) => {
            match fallback_file_metadata(&full) {
                Some((inode_mode, size)) => {
                    const STAT_SIZE: u64 = 144;
                    if !crate::syscall::validate_user_ptr(stat_ptr, STAT_SIZE) {
                        return EFAULT;
                    }
                    write_stat_buf(stat_ptr, mode_for_stat(inode_mode), size);
                    SUCCESS
                }
                None => ENOENT,
            }
        }
        Err(errno) => errno,
    }
}

/// Faccessat システムコール
pub fn faccessat(dirfd: i64, path_ptr: u64, _mode: u64, _flags: u64) -> u64 {
    use super::types::ENOENT;
    const AT_FDCWD: i64 = -100;
    if path_ptr == 0 {
        return EINVAL;
    }
    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let resolved = if dirfd == AT_FDCWD || path.starts_with('/') {
        normalize_path(&path)
    } else {
        let pid = match current_process_id_raw() {
            Some(p) => p,
            None => return EBADF,
        };
        let idx = dirfd as usize;
        if idx >= PROCESS_MAX_FDS {
            return EBADF;
        }
        match with_fd_table(current_process_id_raw().unwrap_or(0), |t| {
            t.get(idx).and_then(|fh| fh.dir_path.clone())
        }) {
            Some(Some(d)) => {
                normalize_path(&alloc::format!("{}/{}", d.trim_end_matches('/'), path))
            }
            _ => return EBADF,
        }
    };
    match stat_path_via_fs_service(&resolved) {
        Ok(_) => SUCCESS,
        Err(errno) if should_fallback_to_initfs(errno) => {
            if fallback_file_metadata(&resolved).is_some() {
                SUCCESS
            } else {
                ENOENT
            }
        }
        Err(errno) => errno,
    }
}

/// statfs システムコール（最小実装）
///
/// Linux x86_64 の `struct statfs` (120 bytes) を埋めて返す。
pub fn statfs(path_ptr: u64, buf_ptr: u64) -> u64 {
    const STATFS_SIZE: u64 = 120;
    if path_ptr == 0 || buf_ptr == 0 {
        return EINVAL;
    }
    if !crate::syscall::validate_user_ptr(buf_ptr, STATFS_SIZE) {
        return EFAULT;
    }

    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };
    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let resolved = resolve_path(pid, &path);
    if stat_path_via_fs_service(&resolved).is_err() && fallback_file_metadata(&resolved).is_none() {
        return ENOENT;
    }

    // struct statfs {
    //   long f_type, f_bsize, f_blocks, f_bfree, f_bavail, f_files, f_ffree;
    //   fsid_t f_fsid; long f_namelen, f_frsize, f_flags, f_spare[4];
    // }
    crate::syscall::with_user_memory_access(|| unsafe {
        let p = buf_ptr as *mut u64;
        core::ptr::write_bytes(buf_ptr as *mut u8, 0, STATFS_SIZE as usize);
        core::ptr::write_unaligned(p.add(0), 0xEF53); // ext2 magic
        core::ptr::write_unaligned(p.add(1), 4096); // f_bsize
        core::ptr::write_unaligned(p.add(8), 255); // f_namelen
        core::ptr::write_unaligned(p.add(9), 4096); // f_frsize
    });
    SUCCESS
}

/// readlinkat システムコール（最小実装）
///
/// `/proc/self/exe` と `/proc/self/cwd` のみをサポートする。
pub fn readlinkat(dirfd: i64, path_ptr: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    const AT_FDCWD: i64 = -100;
    if path_ptr == 0 || buf_ptr == 0 || buf_len == 0 {
        return EINVAL;
    }
    if !crate::syscall::validate_user_ptr(buf_ptr, buf_len) {
        return EFAULT;
    }
    let raw = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let path = if raw.starts_with('/') || dirfd == AT_FDCWD {
        normalize_path(&raw)
    } else {
        // 最小実装: dirfd 相対は未対応
        return EBADF;
    };

    let pid = match current_process_id_raw() {
        Some(p) => crate::task::ids::ProcessId::from_u64(p),
        None => return EBADF,
    };
    let target = if path == "/proc/self/exe" {
        match crate::task::with_process(pid, |p| String::from(p.name())) {
            Some(name) if name.starts_with('/') => name,
            Some(name) => alloc::format!("/{}", name),
            None => return ESRCH,
        }
    } else if path == "/proc/self/cwd" {
        match crate::task::with_process(pid, |p| String::from(p.cwd())) {
            Some(cwd) => cwd,
            None => return ESRCH,
        }
    } else {
        return ENOENT;
    };

    let bytes = target.as_bytes();
    let copy_len = core::cmp::min(bytes.len(), buf_len as usize);
    if let Err(errno) = crate::syscall::copy_to_user(buf_ptr, &bytes[..copy_len]) {
        return errno;
    }
    copy_len as u64
}

/// Getdents64 システムコール
///
/// struct linux_dirent64 形式でエントリをバッファに書き込む。
/// - d_ino (8), d_off (8), d_reclen (2), d_type (1), d_name (可変長, null終端)
/// - レコードは 8 バイトアラインメント
/// FD の `pos` をエントリインデックスとして使用する。
pub fn getdents64(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_ptr == 0 || buf_len == 0 {
        return EINVAL;
    }
    if !crate::syscall::validate_user_ptr(buf_ptr, buf_len) {
        return EFAULT;
    }
    if fd < FD_BASE as u64 {
        return EBADF;
    }
    let idx = fd as usize;
    if idx >= PROCESS_MAX_FDS {
        return EBADF;
    }
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };

    // ディレクトリパスと現在の読み取り位置を取得
    let (dir_path, start_pos, is_remote, fd_remote) = match with_fd_table(pid, |t| {
        t.get(idx)
            .map(|fh| (fh.dir_path.clone(), fh.pos, fh.is_remote, fh.fd_remote))
    }) {
        Some(Some((Some(p), pos, is_remote, fd_remote))) => (p, pos, is_remote, fd_remote),
        // リモート fd は dir_path が None でも fd_remote で readdir できる
        Some(Some((None, pos, true, fd_remote))) => (String::new(), pos, true, fd_remote),
        _ => return EBADF,
    };

    let entries = if is_remote {
        // 大きなディレクトリでの切り詰めを避けるため、オフセット付きで分割取得する。
        let mut all: Vec<(String, u8)> = Vec::new();
        let mut chunk = [0u8; FS_DATA_MAX];
        let mut cursor = 0usize;
        let mut safety = 0usize;
        loop {
            safety = safety.saturating_add(1);
            if safety > 4096 {
                return EIO;
            }
            let (n, next) = match readdir_chunk_via_fs_service(fd_remote, cursor, &mut chunk) {
                Ok(v) => v,
                Err(e) => return e,
            };
            if n == 0 {
                break;
            }
            let parsed = parse_readdir_typed(&chunk[..n]);
            for entry in parsed {
                all.push(entry);
            }
            if next <= cursor {
                break;
            }
            cursor = next;
        }
        all
    } else {
        match fallback_readdir(&dir_path) {
            Some(e) => e
                .into_iter()
                // d_type の判定で追加 stat を打つとカーネルモジュール呼び出し回数が増え不安定化するため、
                // ここでは DT_UNKNOWN(0) を返して利用側のフォールバックに任せる。
                .map(|name| (name, 0u8))
                .collect(),
            None => return EINVAL,
        }
    };

    let mut written: usize = 0;
    let mut new_pos = start_pos;

    // "." と ".." を先頭に追加
    let dot_entries: [(&str, u8); 2] = [(".", 4u8), ("..", 4u8)];
    let all_entries: Vec<(alloc::string::String, u8)> = {
        let mut v: Vec<(alloc::string::String, u8)> = dot_entries
            .iter()
            .map(|(n, t)| (alloc::string::String::from(*n), *t))
            .collect();
        for (name, dtype) in &entries {
            v.push((name.clone(), *dtype));
        }
        v
    };

    crate::syscall::with_user_memory_access(|| {
        for (i, (name, dtype)) in all_entries.iter().enumerate().skip(start_pos) {
            let name_bytes = name.as_bytes();
            let name_len = name_bytes.len() + 1; // null 終端含む
                                                 // d_ino(8) + d_off(8) + d_reclen(2) + d_type(1) + d_name
            let raw_size = 8 + 8 + 2 + 1 + name_len;
            let reclen = (raw_size + 7) & !7usize; // 8 バイトアライン
            if written + reclen > buf_len as usize {
                break;
            }
            let entry_ptr = (buf_ptr + written as u64) as *mut u8;
            unsafe {
                let buf = core::slice::from_raw_parts_mut(entry_ptr, reclen);
                buf.fill(0);
                // d_ino = i+1
                buf[0..8].copy_from_slice(&((i as u64 + 1).to_ne_bytes()));
                // d_off = next position
                let next_off = (i + 1) as u64;
                buf[8..16].copy_from_slice(&next_off.to_ne_bytes());
                // d_reclen
                buf[16..18].copy_from_slice(&(reclen as u16).to_ne_bytes());
                // d_type
                buf[18] = *dtype;
                // d_name (null terminated)
                buf[19..19 + name_bytes.len()].copy_from_slice(name_bytes);
                buf[19 + name_bytes.len()] = 0;
            }
            written += reclen;
            new_pos = i + 1;
        }
    });

    // FD の pos を更新する
    with_fd_table_mut(pid, |t| {
        if let Some(fh) = t.get_mut(idx) {
            fh.pos = new_pos;
        }
    });

    written as u64
}
