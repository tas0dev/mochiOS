//! I/O関連のシステムコール

use super::types::{EBADF, EFAULT, EINVAL, SUCCESS};
use crate::util::console;
use crate::{debug, error, info, warn};

/// 標準出力のファイルディスクリプタ
const STDOUT_FD: u64 = 1;
/// 標準エラー出力のファイルディスクリプタ
const STDERR_FD: u64 = 2;
const IOVEC_SIZE: u64 = 16;
const IOV_MAX: u64 = 1024;

#[inline]
fn is_tty_path(path: Option<&str>) -> bool {
    match path {
        Some(p) => crate::syscall::fs::is_tty_like_path(p),
        None => false,
    }
}

/// 現在のプロセスの親プロセスのメインスレッドIDを返す
fn get_parent_thread_id() -> Option<u64> {
    let tid = crate::task::current_thread_id()?;
    let pid = crate::task::with_thread(tid, |t| t.process_id())?;
    let parent_pid = crate::task::with_process(pid, |p| p.parent_id())??;
    let mut parent_tid: Option<u64> = None;
    crate::task::for_each_thread(|t| {
        if parent_tid.is_none() && t.process_id() == parent_pid {
            parent_tid = Some(t.id().as_u64());
        }
    });
    parent_tid
}

/// Writeシステムコール
///
/// # 引数
/// - `fd`: ファイルディスクリプタ (1=stdout, 2=stderr, >=3=ファイル/パイプ)
/// - `buf_ptr`: 書き込むデータのポインタ
/// - `len`: 書き込むデータの長さ
///
/// # 戻り値
/// 書き込んだバイト数、またはエラーコード
pub fn write(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    debug!("write: fd={}, buf_ptr={:#x}, len={}", fd, buf_ptr, len);

    if len == 0 {
        return SUCCESS;
    }
    if buf_ptr == 0 {
        return EFAULT;
    }

    // fd >= 3: パイプ書き込みを試みる
    if fd >= 3 {
        return write_fd(fd, buf_ptr, len);
    }

    if fd != STDOUT_FD && fd != STDERR_FD {
        return EBADF;
    }

    let mut buf = alloc::vec![0u8; len as usize];
    if let Err(err) = crate::syscall::copy_from_user(buf_ptr, &mut buf) {
        return err;
    }

    // シリアルには常に出力する（デバッグ用）
    x86_64::instructions::interrupts::without_interrupts(|| {
        use core::fmt::Write;
        let mut serial = console::SERIAL.lock();
        for &byte in &buf {
            serial.send_byte(byte);
        }
    });

    // 親プロセス（シェル）が存在すればIPCで転送して描画させる
    let mut sent_chunks: usize = 0;
    let mut failed_chunks: usize = 0;
    let parent_tid = get_parent_thread_id();
    if let Some(parent_tid) = parent_tid {
        const CHUNK: usize = 512;
        const IPC_SEND_RETRY: usize = 64;
        let mut offset = 0;
        while offset < buf.len() {
            let end = core::cmp::min(offset + CHUNK, buf.len());
            let mut sent = false;
            for _ in 0..IPC_SEND_RETRY {
                if crate::syscall::ipc::send_from_kernel(parent_tid, &buf[offset..end]) {
                    sent = true;
                    break;
                }
                crate::task::yield_now();
            }
            if sent {
                sent_chunks += 1;
            } else {
                failed_chunks += 1;
            }
            offset = end;
        }
    }
    len
}

/// Writevシステムコール
///
/// iov 配列を順に処理し、内部的に `write` を呼び出す。
pub fn writev(fd: u64, iov_ptr: u64, iovcnt: u64) -> u64 {
    if iovcnt == 0 {
        return SUCCESS;
    }
    if iov_ptr == 0 {
        return EFAULT;
    }
    if iovcnt > IOV_MAX {
        return EINVAL;
    }

    let table_bytes = match iovcnt.checked_mul(IOVEC_SIZE) {
        Some(n) => n,
        None => return EINVAL,
    };
    if !super::validate_user_ptr(iov_ptr, table_bytes) {
        return EFAULT;
    }

    let mut total_written: u64 = 0;
    for i in 0..iovcnt {
        let off = match i.checked_mul(IOVEC_SIZE) {
            Some(v) => v,
            None => return EINVAL,
        };
        let entry_ptr = match iov_ptr.checked_add(off) {
            Some(v) => v,
            None => return EFAULT,
        };

        let mut entry = [0u8; IOVEC_SIZE as usize];
        if let Err(err) = crate::syscall::copy_from_user(entry_ptr, &mut entry) {
            return if total_written > 0 {
                total_written
            } else {
                err
            };
        }

        let mut base_bytes = [0u8; 8];
        let mut len_bytes = [0u8; 8];
        base_bytes.copy_from_slice(&entry[0..8]);
        len_bytes.copy_from_slice(&entry[8..16]);
        let base = u64::from_ne_bytes(base_bytes);
        let len = u64::from_ne_bytes(len_bytes);

        if len == 0 {
            continue;
        }
        if base == 0 {
            return if total_written > 0 {
                total_written
            } else {
                EFAULT
            };
        }

        let wrote = write(fd, base, len);
        if (wrote as i64) < 0 {
            return if total_written > 0 {
                total_written
            } else {
                wrote
            };
        }

        total_written = match total_written.checked_add(wrote) {
            Some(v) => v,
            None => return EINVAL,
        };

        if wrote < len {
            break;
        }
    }

    total_written
}

/// Readvシステムコール
///
/// iov 配列を順に処理し、内部的に `read` を呼び出す。
pub fn readv(fd: u64, iov_ptr: u64, iovcnt: u64) -> u64 {
    if iovcnt == 0 {
        return SUCCESS;
    }
    if iov_ptr == 0 {
        return EFAULT;
    }
    if iovcnt > IOV_MAX {
        return EINVAL;
    }

    let table_bytes = match iovcnt.checked_mul(IOVEC_SIZE) {
        Some(n) => n,
        None => return EINVAL,
    };
    if !super::validate_user_ptr(iov_ptr, table_bytes) {
        return EFAULT;
    }

    let mut total_read: u64 = 0;
    for i in 0..iovcnt {
        let off = match i.checked_mul(IOVEC_SIZE) {
            Some(v) => v,
            None => return EINVAL,
        };
        let entry_ptr = match iov_ptr.checked_add(off) {
            Some(v) => v,
            None => return EFAULT,
        };

        let mut entry = [0u8; IOVEC_SIZE as usize];
        if let Err(err) = crate::syscall::copy_from_user(entry_ptr, &mut entry) {
            return if total_read > 0 { total_read } else { err };
        }

        let mut base_bytes = [0u8; 8];
        let mut len_bytes = [0u8; 8];
        base_bytes.copy_from_slice(&entry[0..8]);
        len_bytes.copy_from_slice(&entry[8..16]);
        let base = u64::from_ne_bytes(base_bytes);
        let len = u64::from_ne_bytes(len_bytes);

        if len == 0 {
            continue;
        }
        if base == 0 {
            return if total_read > 0 { total_read } else { EFAULT };
        }

        let n = read(fd, base, len);
        if (n as i64) < 0 {
            return if total_read > 0 { total_read } else { n };
        }
        total_read = match total_read.checked_add(n) {
            Some(v) => v,
            None => return EINVAL,
        };
        if n < len {
            break;
        }
    }
    total_read
}

/// fd >= 3 への書き込み（パイプ書き込み端か通常ファイルへの書き込み）
fn write_fd(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    let pid = match crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
    {
        Some(p) => p,
        None => return EBADF,
    };

    let idx = fd as usize;
    // パイプ/TTY かどうか確認
    let fd_info = crate::task::with_process(pid, |p| {
        p.fd_table().get(idx).map(|fh| {
            (
                fh.pipe_id,
                fh.pipe_write,
                is_tty_path(fh.dir_path.as_deref()),
            )
        })
    })
    .flatten();

    match fd_info {
        Some((_, _, true)) => write(STDOUT_FD, buf_ptr, len),
        Some((Some(pipe_id), true, false)) => {
            // パイプ書き込み端
            if !super::validate_user_ptr(buf_ptr, len) {
                return EFAULT;
            }
            let mut buf = alloc::vec![0u8; len as usize];
            if let Err(e) = crate::syscall::copy_from_user(buf_ptr, &mut buf) {
                return e;
            }
            match crate::syscall::pipe::pipe_write(pipe_id, &buf) {
                Ok(n) => n as u64,
                Err(e) => e,
            }
        }
        Some((None, _, false)) => crate::syscall::fs::write(fd, buf_ptr, len),
        Some((Some(_), false, false)) => EBADF,
        None => EBADF,
    }
}

/// Readシステムコール
/// - fd == 0 の場合はキーボードからブロッキングで読み取る
/// - fd >= 3 の場合はパイプ or initfs から開かれたファイルを読み取る（fs::read / pipe に委譲）
pub fn read(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    use super::types::EFAULT;

    if buf_ptr == 0 {
        return EFAULT;
    }
    if len == 0 {
        return 0;
    }

    if fd == 0 {
        return crate::syscall::tty::read_stdin(buf_ptr, len);
    }

    if fd >= 3 {
        return read_fd(fd, buf_ptr, len);
    }

    // fd=1,2 への read は無効
    super::types::EBADF
}

/// fd >= 3 からの読み取り（パイプ読み込み端 or 通常ファイル）
fn read_fd(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    let pid = match crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
    {
        Some(p) => p,
        None => return EBADF,
    };

    let idx = fd as usize;
    let fd_info = crate::task::with_process(pid, |p| {
        p.fd_table().get(idx).map(|fh| {
            (
                fh.pipe_id,
                fh.pipe_write,
                is_tty_path(fh.dir_path.as_deref()),
            )
        })
    })
    .flatten();

    match fd_info {
        Some((_, _, true)) => crate::syscall::tty::read_stdin(buf_ptr, len),
        Some((Some(pipe_id), false, false)) => {
            // パイプ読み込み端: ブロッキング読み取り
            if !super::validate_user_ptr(buf_ptr, len) {
                return EFAULT;
            }
            let mut buf = alloc::vec![0u8; len as usize];
            let n = crate::syscall::pipe::pipe_read_blocking(pipe_id, &mut buf);
            if n > 0 {
                crate::syscall::with_user_memory_access(|| unsafe {
                    core::ptr::copy_nonoverlapping(buf.as_ptr(), buf_ptr as *mut u8, n);
                });
            }
            n as u64
        }
        Some(_) => {
            // 通常ファイル
            crate::syscall::fs::read(fd, buf_ptr, len)
        }
        None => EBADF,
    }
}

/// Logシステムコール
///
/// カーネルログにメッセージを書き込む
/// # 引数
/// msg: メッセージ
/// len: メッセージの長さ
/// level: ログレベル（0=ERROR、1=WARNING、2=INFO、3=DEBUG）
///
/// # 戻り値
/// 成功時はSUCCESS、エラー時はエラーコード
pub fn log(msg: u64, len: u64, level: u64) -> u64 {
    if msg == 0 || len == 0 {
        return super::types::EINVAL;
    }

    let mut copied = alloc::vec![0u8; len as usize];
    if let Err(err) = crate::syscall::copy_from_user(msg, &mut copied) {
        return err;
    }

    let msg = match core::str::from_utf8(&copied) {
        Ok(s) => s,
        Err(_) => return super::types::EINVAL,
    };

    match level {
        0 => error!("{}", msg),
        1 => warn!("{}", msg),
        2 => info!("{}", msg),
        3 => debug!("{}", msg),
        _ => return super::types::EINVAL,
    }
    SUCCESS
}
