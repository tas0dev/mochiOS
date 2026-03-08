//! ファイルシステム関連のシステムコール

use super::types::{EBADF, EFAULT, EINVAL, ENOENT, ENOSYS, SUCCESS};
use crate::interrupt::spinlock::SpinLock;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

const MAX_FDS: usize = 64;
const FD_BASE: usize = 3; // 0,1,2 は stdio 用に予約

/// ユーザ空間から開かれたファイルを保持するハンドル
#[repr(C)]
struct FileHandle {
    owner_pid: u64,
    data: Box<[u8]>,
    pos: usize,
}

// ファイルディスクリプタテーブル: 0 == 未使用, それ以外は Box<FileHandle> の生ポインタ (u64)
static FD_TABLE: SpinLock<[u64; MAX_FDS]> = SpinLock::new([0u64; MAX_FDS]);

#[inline]
fn current_process_id_raw() -> Option<u64> {
    crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id().as_u64()))
}

// ユーザー文字列 (null 末尾) を安全にコピーして String にする
fn read_cstring(ptr: u64) -> Result<String, u64> {
    crate::syscall::read_user_cstring(ptr, 1024)
}

/// Openシステムコール (initfs の読み取り専用をサポートする簡易実装)
/// - path はユーザー空間の null 終端文字列ポインタ
/// - flags は無視
pub fn open(path_ptr: u64, _flags: u64) -> u64 {
    let owner_pid = match current_process_id_raw() {
        Some(pid) => pid,
        None => return EBADF,
    };

    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };

    // initfs からファイルを読み込む
    let data_vec = match crate::init::fs::read(&path) {
        Some(d) => d,
        None => return ENOENT,
    };

    // ハンドルを確保して所有権を Box にして登録する
    let handle = Box::new(FileHandle {
        owner_pid,
        data: data_vec.into_boxed_slice(),
        pos: 0,
    });
    let ptr = Box::into_raw(handle) as u64;

    let mut table = FD_TABLE.lock();
    for i in FD_BASE..MAX_FDS {
        if table[i] == 0 {
            table[i] = ptr;
            return i as u64;
        }
    }

    // 空きなし -> 解放してエラー
    unsafe {
        Box::from_raw(ptr as *mut FileHandle);
    }
    ENOSYS
}

/// Closeシステムコール
pub fn close(fd: u64) -> u64 {
    if fd < FD_BASE as u64 {
        return EBADF; // stdin/stdout/stderr は閉じられない
    }
    let idx = fd as usize;
    if idx >= MAX_FDS {
        return EBADF;
    }
    let caller_pid = match current_process_id_raw() {
        Some(pid) => pid,
        None => return EBADF,
    };

    let mut table = FD_TABLE.lock();
    let ptr = table[idx];
    if ptr == 0 {
        return EBADF;
    }
    let owner_pid = unsafe { (*(ptr as *const FileHandle)).owner_pid };
    if owner_pid != caller_pid {
        return EBADF;
    }
    table[idx] = 0;
    drop(table);

    // 所有権を回収して破棄
    unsafe {
        Box::from_raw(ptr as *mut FileHandle);
    }
    SUCCESS
}

/// Seekシステムコール
pub fn seek(fd: u64, offset: i64, whence: u64) -> u64 {
    if fd < FD_BASE as u64 {
        return ENOSYS;
    }
    let idx = fd as usize;
    if idx >= MAX_FDS {
        return EBADF;
    }
    let caller_pid = match current_process_id_raw() {
        Some(pid) => pid,
        None => return EBADF,
    };

    let mut table = FD_TABLE.lock();
    let ptr = table[idx];
    if ptr == 0 {
        return EBADF;
    }

    let fh = unsafe { &mut *(ptr as *mut FileHandle) };
    if fh.owner_pid != caller_pid {
        return EBADF;
    }
    let len = fh.data.len() as i64;
    let new_pos = match whence {
        0 => offset,                 // SEEK_SET
        1 => fh.pos as i64 + offset, // SEEK_CUR
        2 => len + offset,           // SEEK_END
        _ => return EINVAL,
    };
    if new_pos < 0 {
        return EINVAL;
    }
    let new_pos = core::cmp::min(new_pos as usize, fh.data.len());
    fh.pos = new_pos;
    fh.pos as u64
}

/// Fstatシステムコール (簡易実装: 指定されたポインタが0でなければ成功を返す)
pub fn fstat(fd: u64, stat_ptr: u64) -> u64 {
    if stat_ptr == 0 {
        return EFAULT;
    }
    // 互換性のため最小サイズ分をゼロ初期化する
    const MIN_STAT_SIZE: u64 = 64;
    if !crate::syscall::validate_user_ptr(stat_ptr, MIN_STAT_SIZE) {
        return EFAULT;
    }

    let fd_valid = if fd < FD_BASE as u64 {
        // stdin/stdout/stderr
        true
    } else {
        let caller_pid = match current_process_id_raw() {
            Some(pid) => pid,
            None => return EBADF,
        };
        let idx = fd as usize;
        if idx >= MAX_FDS {
            false
        } else {
            let table = FD_TABLE.lock();
            let ptr = table[idx];
            if ptr == 0 {
                false
            } else {
                let owner_pid = unsafe { (*(ptr as *const FileHandle)).owner_pid };
                owner_pid == caller_pid
            }
        }
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
    if crate::init::fs::read(&path).is_some() {
        // stat バッファを最小限ゼロ初期化して返す
        const MIN_STAT_SIZE: u64 = 144; // sizeof(struct stat) on Linux x86_64
        if stat_ptr != 0 && crate::syscall::validate_user_ptr(stat_ptr, MIN_STAT_SIZE) {
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

/// Readdirシステムコール（簡易実装）
/// - 指定された buf_ptr に root ディレクトリのファイル名を改行区切りで書き込む
pub fn readdir(_fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_ptr == 0 || buf_len == 0 {
        return EINVAL;
    }
    // ユーザー空間アドレスの有効性を検証する
    if !crate::syscall::validate_user_ptr(buf_ptr, buf_len) {
        return EFAULT;
    }
    let mut names = Vec::new();
    for e in crate::init::fs::entries() {
        names.push(e.name.to_string());
    }
    let joined = names.join("\n");
    let bytes = joined.as_bytes();
    let to_copy = core::cmp::min(bytes.len(), buf_len as usize);
    crate::syscall::with_user_memory_access(|| unsafe {
        let dst = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, to_copy);
        dst.copy_from_slice(&bytes[..to_copy]);
    });
    to_copy as u64
}

/// Chdirシステムコール（簡易実装）
pub fn chdir(path_ptr: u64) -> u64 {
    if path_ptr == 0 {
        return EINVAL;
    }
    let path = match read_cstring(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    // initfs はルートのみサポートしているため "/" のみを受け入れる
    if path == "/" {
        SUCCESS
    } else {
        ENOENT
    }
}

/// Getcwdシステムコール（簡易実装）
pub fn getcwd(buf_ptr: u64, size: u64) -> u64 {
    if buf_ptr == 0 || size == 0 {
        return EINVAL;
    }
    // ユーザー空間アドレスの有効性を検証する
    if !crate::syscall::validate_user_ptr(buf_ptr, size) {
        return EFAULT;
    }
    let cwd = b"/\0";
    if (size as usize) < cwd.len() {
        return EINVAL;
    }
    crate::syscall::with_user_memory_access(|| unsafe {
        core::ptr::copy_nonoverlapping(cwd.as_ptr(), buf_ptr as *mut u8, cwd.len());
    });
    buf_ptr
}

/// Read: 開かれたファイルからデータを読み込む簡易実装
pub fn read(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if buf_ptr == 0 {
        return EFAULT;
    }
    if len == 0 {
        return 0;
    }
    // ユーザー空間アドレスの有効性を事前に検証する
    if !crate::syscall::validate_user_ptr(buf_ptr, len) {
        return EFAULT;
    }
    if fd < FD_BASE as u64 {
        return EBADF;
    }
    let idx = fd as usize;
    if idx >= MAX_FDS {
        return EBADF;
    }
    let caller_pid = match current_process_id_raw() {
        Some(pid) => pid,
        None => return EBADF,
    };
    // UAF修正: ロックを保持したままFileHandleにアクセスする
    // (ロック保持中はclose()がブロックされるため解放済みメモリアクセスを防ぐ)
    let mut table = FD_TABLE.lock();
    let ptr = table[idx];
    if ptr == 0 {
        return EBADF;
    }

    let fh = unsafe { &mut *(ptr as *mut FileHandle) };
    if fh.owner_pid != caller_pid {
        return EBADF;
    }
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
    drop(table);
    to_read as u64
}
