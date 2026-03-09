//! ファイルシステム関連のシステムコール

use super::types::{EBADF, EFAULT, EINVAL, ENOENT, ENOSYS, SUCCESS};
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use crate::task::fd_table::{FdTable, FileHandle, FD_BASE, O_CLOEXEC, PROCESS_MAX_FDS};

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

fn read_cstring(ptr: u64) -> Result<String, u64> {
    crate::syscall::read_user_cstring(ptr, 1024)
}

/// パスを正規化する（`.` / `..` を解決し重複スラッシュを除去）
fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => { parts.pop(); }
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

    let (data_vec, dir_path) = if crate::init::fs::is_directory(&path) {
        (Vec::new(), Some(path.clone()))
    } else {
        match crate::init::fs::read(&path) {
            Some(d) => (d, None),
            None => return ENOENT,
        }
    };

    let cloexec = (flags & O_CLOEXEC) != 0;
    let handle = alloc::boxed::Box::new(FileHandle {
        data: data_vec.into_boxed_slice(),
        pos: 0,
        dir_path,
    });

    match with_fd_table_mut(owner_pid, |t| t.alloc(handle, cloexec)) {
        Some(Some(fd)) => fd as u64,
        _ => ENOSYS,
    }
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
    match with_fd_table_mut(pid, |t| t.close_fd(idx)) {
        Some(true) => SUCCESS,
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

    let fh_ptr = match with_fd_table(pid, |t| t.get_raw(idx)) {
        Some(Some(ptr)) => ptr,
        _ => return EBADF,
    };
    let fh = unsafe { &mut *fh_ptr };
    let len = fh.data.len() as i64;
    let new_pos = match whence {
        0 => offset,
        1 => fh.pos as i64 + offset,
        2 => len + offset,
        _ => return EINVAL,
    };
    if new_pos < 0 {
        return EINVAL;
    }
    let new_pos = core::cmp::min(new_pos as usize, fh.data.len());
    fh.pos = new_pos;
    fh.pos as u64
}

/// Fstatシステムコール (簡易実装)
pub fn fstat(fd: u64, stat_ptr: u64) -> u64 {
    if stat_ptr == 0 {
        return EFAULT;
    }
    const MIN_STAT_SIZE: u64 = 144;
    if !crate::syscall::validate_user_ptr(stat_ptr, MIN_STAT_SIZE) {
        return EFAULT;
    }

    let fd_valid = if fd < FD_BASE as u64 {
        true
    } else {
        let pid = match current_process_id_raw() {
            Some(p) => p,
            None => return EBADF,
        };
        matches!(
            with_fd_table(pid, |t| t.get_raw(fd as usize)),
            Some(Some(_))
        )
    };
    if !fd_valid {
        return EBADF;
    }

    crate::syscall::with_user_memory_access(|| unsafe {
        core::ptr::write_bytes(stat_ptr as *mut u8, 0, MIN_STAT_SIZE as usize);
    });
    SUCCESS
}

/// Statシステムコール (簡易実装)
pub fn stat(path_ptr: u64, stat_ptr: u64) -> u64 {
    if path_ptr == 0 || stat_ptr == 0 {
        return EINVAL;
    }
    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    if crate::init::fs::read(&path).is_some() || crate::init::fs::is_directory(&path) {
        const MIN_STAT_SIZE: u64 = 144;
        if crate::syscall::validate_user_ptr(stat_ptr, MIN_STAT_SIZE) {
            crate::syscall::with_user_memory_access(|| unsafe {
                core::ptr::write_bytes(stat_ptr as *mut u8, 0, MIN_STAT_SIZE as usize);
            });
        }
        SUCCESS
    } else {
        ENOENT
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

    let dir_path = match with_fd_table(pid, |t| {
        t.get_raw(idx).and_then(|ptr| {
            let fh = unsafe { &*ptr };
            fh.dir_path.clone()
        })
    }) {
        Some(Some(p)) => p,
        _ => return EBADF,
    };

    let names = match crate::init::fs::readdir_path(&dir_path) {
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
    if !crate::init::fs::is_directory(&resolved) {
        return ENOENT;
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

    // PROCESS_TABLE ロックを短く保持して生ポインタを取得する。
    // int 0x80 ハンドラ内では割り込み無効かつ同一プロセス単一スレッドなので
    // ロック解放後もポインタは安全に使用できる。
    let fh_ptr = match with_fd_table(pid, |t| t.get_raw(idx)) {
        Some(Some(ptr)) => ptr,
        _ => return EBADF,
    };
    let fh = unsafe { &mut *fh_ptr };
    let avail = fh.data.len().saturating_sub(fh.pos);
    if avail == 0 {
        return 0;
    }
    let to_read = core::cmp::min(avail, len as usize);
    crate::syscall::with_user_memory_access(|| unsafe {
        let dst = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, to_read);
        dst.copy_from_slice(&fh.data[fh.pos..fh.pos + to_read]);
    });
    fh.pos += to_read;
    to_read as u64
}
