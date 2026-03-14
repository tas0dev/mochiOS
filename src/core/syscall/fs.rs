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

#[inline]
fn is_process_busybox(pid_raw: u64) -> bool {
    let pid = crate::task::ids::ProcessId::from_u64(pid_raw);
    crate::task::with_process(pid, |p| p.name().ends_with("busybox.elf")).unwrap_or(false)
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
    let busybox = is_process_busybox(owner_pid);
    if busybox {
        crate::info!("busybox open: path='{}', flags={:#x}", path, flags);
    }

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
        pipe_id: None,
        pipe_write: false,
    });

    let ret = match with_fd_table_mut(owner_pid, |t| t.alloc(handle, cloexec)) {
        Some(Some(fd)) => fd as u64,
        _ => ENOSYS,
    };
    if busybox {
        crate::info!("busybox open -> {}", ret);
    }
    ret
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
    let ret = match with_fd_table_mut(pid, |t| t.close_fd(idx)) {
        Some(true) => SUCCESS,
        _ => EBADF,
    };
    if is_process_busybox(pid) {
        crate::info!("busybox close: fd={}, ret={:#x}", fd, ret);
    }
    ret
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

    // FileHandle から (size, is_dir) を取得する
    let file_info = with_fd_table(pid, |t| {
        t.get_raw(idx).map(|ptr| {
            let fh = unsafe { &*ptr };
            (fh.data.len() as u64, fh.dir_path.is_some())
        })
    });
    let (size, is_dir) = match file_info {
        Some(Some(v)) => v,
        _ => return EBADF,
    };
    // S_IFREG = 0x8000, S_IFDIR = 0x4000
    let mode = if is_dir { 0x4000u32 | 0o755 } else { 0x8000u32 | 0o755 };
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
    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let pid_raw = current_process_id_raw();
    let resolved = if let Some(p) = pid_raw {
        if path.starts_with('/') {
            path
        } else {
            let pid = crate::task::ids::ProcessId::from_u64(p);
            let cwd = crate::task::with_process(pid, |proc| {
                let mut s = alloc::string::String::new();
                s.push_str(proc.cwd());
                s
            }).unwrap_or_else(|| "/".to_string());
            let joined = alloc::format!("{}/{}", cwd.trim_end_matches('/'), path);
            // 正規化は簡易: 連続スラッシュのみ処理
            joined
        }
    } else {
        path
    };

    match crate::init::fs::file_metadata(&resolved) {
        Some((inode_mode, size)) => {
            // ext2 inode mode をそのまま使用（S_IFREG/S_IFDIR ビットを保持）
            let mode = inode_mode as u32;
            // パーミッションビットが 0 の場合はデフォルト値を設定
            let perm = mode & 0o777;
            let mode = if perm == 0 { mode | 0o755 } else { mode };
            write_stat_buf(stat_ptr, mode, size);
            SUCCESS
        }
        None => ENOENT,
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
        F_GETFD => {
            match with_fd_table(pid, |t| t.get_flags(idx)) {
                Some(Some(flags)) => flags as u64,
                _ => EBADF,
            }
        }
        F_SETFD => {
            let cloexec = (arg & 1) != 0;
            let new_flags = if cloexec { FD_CLOEXEC } else { 0 };
            match with_fd_table_mut(pid, |t| t.set_flags(idx, new_flags)) {
                Some(true) => SUCCESS,
                _ => EBADF,
            }
        }
        F_GETFL => 0,    // O_RDONLY スタブ
        F_SETFL => SUCCESS,
        _ => EINVAL,
    }
}

/// Dup システムコール: FD を複製して最小の空き番号に割り当てる
pub fn dup(fd: u64) -> u64 {
    if fd < FD_BASE as u64 {
        // stdin/stdout/stderr の複製は対応しない（スタブ: EBADF）
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

    // 既存エントリをクローンして新しい FD を割り当てる
    let cloned = with_fd_table(pid, |t| {
        t.get_raw(idx).map(|ptr| {
            let fh = unsafe { &*ptr };
            alloc::boxed::Box::new(FileHandle {
                data: fh.data.clone(),
                pos: fh.pos,
                dir_path: fh.dir_path.clone(),
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
        return match with_fd_table(pid, |t| t.get_raw(old_fd as usize)) {
            Some(Some(_)) => old_fd,
            _ => EBADF,
        };
    }

    let old_idx = old_fd as usize;
    if old_idx >= PROCESS_MAX_FDS {
        return EBADF;
    }
    let new_idx = new_fd as usize;
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };

    // old_fd のクローンを作成
    let cloned = with_fd_table(pid, |t| {
        t.get_raw(old_idx).map(|ptr| {
            let fh = unsafe { &*ptr };
            alloc::boxed::Box::new(FileHandle {
                data: fh.data.clone(),
                pos: fh.pos,
                dir_path: fh.dir_path.clone(),
                pipe_id: fh.pipe_id,
                pipe_write: fh.pipe_write,
            })
        })
    });
    let new_handle = match cloned {
        Some(Some(h)) => h,
        _ => return EBADF,
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
    let dir_path = match with_fd_table(pid, |t| {
        t.get_raw(idx).and_then(|ptr| {
            let fh = unsafe { &*ptr };
            fh.dir_path.clone()
        })
    }) {
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

    // full_path を持つ一時ポインタを使って open する代わりに直接処理
    let (data_vec, final_dir_path) = if crate::init::fs::is_directory(&full_path) {
        (Vec::new(), Some(full_path.clone()))
    } else {
        match crate::init::fs::read(&full_path) {
            Some(d) => (d, None),
            None => return ENOENT,
        }
    };

    let cloexec = (flags & O_CLOEXEC) != 0;
    let handle = alloc::boxed::Box::new(FileHandle {
        data: data_vec.into_boxed_slice(),
        pos: 0,
        dir_path: final_dir_path,
        pipe_id: None,
        pipe_write: false,
    });
    match with_fd_table_mut(pid, |t| t.alloc(handle, cloexec)) {
        Some(Some(fd)) => fd as u64,
        _ => ENOSYS,
    }
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
    if idx >= PROCESS_MAX_FDS { return EBADF; }
    let dir_path = match with_fd_table(pid, |t| {
        t.get_raw(idx).and_then(|ptr| unsafe { (*ptr).dir_path.clone() })
    }) {
        Some(Some(p)) => p,
        _ => return EBADF,
    };
    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let full = if path.starts_with('/') { path } else {
        alloc::format!("{}/{}", dir_path.trim_end_matches('/'), path)
    };
    match crate::init::fs::file_metadata(&full) {
        Some((inode_mode, size)) => {
            const STAT_SIZE: u64 = 144;
            if !crate::syscall::validate_user_ptr(stat_ptr, STAT_SIZE) { return EFAULT; }
            let perm = (inode_mode as u32) & 0o777;
            let mode = if perm == 0 { inode_mode as u32 | 0o755 } else { inode_mode as u32 };
            write_stat_buf(stat_ptr, mode, size);
            SUCCESS
        }
        None => ENOENT,
    }
}

/// Faccessat システムコール
pub fn faccessat(dirfd: i64, path_ptr: u64, _mode: u64, _flags: u64) -> u64 {
    use super::types::ENOENT;
    const AT_FDCWD: i64 = -100;
    if path_ptr == 0 { return EINVAL; }
    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let resolved = if dirfd == AT_FDCWD || path.starts_with('/') {
        path
    } else {
        let pid = match current_process_id_raw() {
            Some(p) => p,
            None => return EBADF,
        };
        let idx = dirfd as usize;
        if idx >= PROCESS_MAX_FDS { return EBADF; }
        match with_fd_table(current_process_id_raw().unwrap_or(0), |t| {
            t.get_raw(idx).and_then(|ptr| unsafe { (*ptr).dir_path.clone() })
        }) {
            Some(Some(d)) => alloc::format!("{}/{}", d.trim_end_matches('/'), path),
            _ => return EBADF,
        }
    };
    if crate::init::fs::file_metadata(&resolved).is_some() { SUCCESS } else { ENOENT }
}

/// Getdents64 システムコール
///
/// struct linux_dirent64 形式でエントリをバッファに書き込む。
/// - d_ino (8), d_off (8), d_reclen (2), d_type (1), d_name (可変長, null終端)
/// - レコードは 8 バイトアラインメント
/// FD の `pos` をエントリインデックスとして使用する。
pub fn getdents64(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_ptr == 0 || buf_len == 0 { return EINVAL; }
    if !crate::syscall::validate_user_ptr(buf_ptr, buf_len) { return EFAULT; }
    if fd < FD_BASE as u64 { return EBADF; }
    let idx = fd as usize;
    if idx >= PROCESS_MAX_FDS { return EBADF; }
    let pid = match current_process_id_raw() {
        Some(p) => p,
        None => return EBADF,
    };

    // ディレクトリパスと現在の読み取り位置を取得
    let (dir_path, start_pos) = match with_fd_table(pid, |t| {
        t.get_raw(idx).map(|ptr| {
            let fh = unsafe { &*ptr };
            (fh.dir_path.clone(), fh.pos)
        })
    }) {
        Some(Some((Some(p), pos))) => (p, pos),
        _ => {
            if is_process_busybox(pid) {
                crate::info!("busybox getdents64: fd={} is invalid or not a directory", fd);
            }
            return EBADF;
        }
    };

    let entries = match crate::init::fs::readdir_path(&dir_path) {
        Some(e) => e,
        None => return EINVAL,
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
        for name in &entries {
            // ディレクトリかファイルかを判定
            let child_path = alloc::format!("{}/{}", dir_path.trim_end_matches('/'), name);
            let dtype = if crate::init::fs::is_directory(&child_path) { 4u8 } else { 8u8 };
            v.push((name.clone(), dtype));
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
        if let Some(ptr) = t.get_raw(idx) {
            unsafe { (*ptr).pos = new_pos; }
        }
    });

    if is_process_busybox(pid) {
        crate::info!(
            "busybox getdents64: fd={}, start_pos={}, entries={}, written={}",
            fd,
            start_pos,
            all_entries.len(),
            written
        );
    }

    written as u64
}
