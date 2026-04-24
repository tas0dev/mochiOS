//! パイプの実装
//!
//! グローバルな PIPE_TABLE を使い、FD テーブルの FileHandle から参照する。
//! 読み込み端がブロックする場合は KEYBOARD_WAITER と同様に wake_thread を使う。

use crate::interrupt::spinlock::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

/// パイプバッファのサイズ（64 KiB）
const PIPE_BUF_SIZE: usize = 65536;

/// 同時に存在できるパイプの最大数
const MAX_PIPES: usize = 64;

/// パイプバッファ
pub struct PipeBuffer {
    buf: [u8; PIPE_BUF_SIZE],
    /// リングバッファの書き込み位置
    write_pos: usize,
    /// リングバッファの読み込み位置
    read_pos: usize,
    /// バッファ内のデータ量
    len: usize,
    /// 読み込み端を開いている FD の参照カウント
    pub read_refs: u32,
    /// 書き込み端を開いている FD の参照カウント
    pub write_refs: u32,
    /// 読み込み待ちスレッド ID（ブロッキング読み込み用）
    waiter: AtomicU64,
}

impl PipeBuffer {
    const fn new() -> Self {
        Self {
            buf: [0u8; PIPE_BUF_SIZE],
            write_pos: 0,
            read_pos: 0,
            len: 0,
            read_refs: 0,
            write_refs: 0,
            waiter: AtomicU64::new(0),
        }
    }

    /// バッファにデータを書き込む。書き込んだバイト数を返す。
    pub fn write_bytes(&mut self, data: &[u8]) -> usize {
        let avail = PIPE_BUF_SIZE - self.len;
        let to_write = core::cmp::min(data.len(), avail);
        for i in 0..to_write {
            self.buf[self.write_pos] = data[i];
            self.write_pos = (self.write_pos + 1) % PIPE_BUF_SIZE;
        }
        self.len += to_write;
        to_write
    }

    /// バッファからデータを読み取る。読み取ったバイト数を返す。
    pub fn read_bytes(&mut self, dst: &mut [u8]) -> usize {
        let to_read = core::cmp::min(dst.len(), self.len);
        for i in 0..to_read {
            dst[i] = self.buf[self.read_pos];
            self.read_pos = (self.read_pos + 1) % PIPE_BUF_SIZE;
        }
        self.len -= to_read;
        to_read
    }

    pub fn available(&self) -> usize {
        self.len
    }
    pub fn is_full(&self) -> bool {
        self.len == PIPE_BUF_SIZE
    }

    /// 読み込み待ちスレッドを登録する
    pub fn set_waiter(&self, tid: u64) {
        self.waiter.store(tid, Ordering::SeqCst);
    }

    /// 読み込み待ちスレッドを解除する
    pub fn clear_waiter(&self) {
        self.waiter.store(0, Ordering::SeqCst);
    }

    /// 待機中のスレッドを起こす
    pub fn wake_reader(&self) {
        let tid = self.waiter.load(Ordering::SeqCst);
        if tid != 0 {
            crate::task::wake_thread(crate::task::ids::ThreadId::from_u64(tid));
        }
    }
}

/// グローバルパイプテーブル
/// SpinLock でガードした Option<PipeBuffer> の配列
static PIPE_TABLE: SpinLock<[Option<PipeBuffer>; MAX_PIPES]> = {
    const INIT: Option<PipeBuffer> = None;
    SpinLock::new([INIT; MAX_PIPES])
};

/// 新しいパイプを確保してパイプ ID を返す。失敗した場合は None。
pub fn alloc_pipe() -> Option<usize> {
    let mut table = PIPE_TABLE.lock();
    for (i, slot) in table.iter_mut().enumerate() {
        if slot.is_none() {
            let mut pb = PipeBuffer::new();
            pb.read_refs = 1;
            pb.write_refs = 1;
            *slot = Some(pb);
            return Some(i);
        }
    }
    None
}

/// パイプの書き込み端を閉じる（write_refs をデクリメントし 0 になったら待機中スレッドを起こす）
pub fn close_write_end(id: usize) {
    let mut table = PIPE_TABLE.lock();
    if let Some(Some(pb)) = table.get_mut(id) {
        if pb.write_refs > 0 {
            pb.write_refs -= 1;
        }
        if pb.write_refs == 0 {
            pb.wake_reader();
        }
        if pb.read_refs == 0 && pb.write_refs == 0 {
            table[id] = None;
        }
    }
}

/// パイプの読み込み端を閉じる
pub fn close_read_end(id: usize) {
    let mut table = PIPE_TABLE.lock();
    if let Some(Some(pb)) = table.get_mut(id) {
        if pb.read_refs > 0 {
            pb.read_refs -= 1;
        }
        if pb.read_refs == 0 && pb.write_refs == 0 {
            table[id] = None;
        }
    }
}

/// パイプに書き込む。バッファが満杯の場合は EAGAIN（簡易実装: ノンブロッキング）。
/// 書き込んだバイト数を返す。
pub fn pipe_write(id: usize, data: &[u8]) -> Result<usize, u64> {
    use super::types::{EAGAIN, EPIPE};
    let mut table = PIPE_TABLE.lock();
    match table.get_mut(id).and_then(|s| s.as_mut()) {
        None => Err(EPIPE),
        Some(pb) => {
            if pb.read_refs == 0 {
                return Err(EPIPE);
            }
            if pb.is_full() {
                return Err(EAGAIN);
            }
            let n = pb.write_bytes(data);
            pb.wake_reader();
            Ok(n)
        }
    }
}

/// パイプから読み取る（ブロッキング）。
/// データがあれば即座に返し、なければ書き込み端が閉じられるまで待機する。
pub fn pipe_read_blocking(id: usize, dst: &mut [u8]) -> usize {
    loop {
        {
            let mut table = PIPE_TABLE.lock();
            if let Some(Some(pb)) = table.get_mut(id) {
                if pb.available() > 0 {
                    return pb.read_bytes(dst);
                }
                if pb.write_refs == 0 {
                    // 書き込み端がすべて閉じられ、データなし → EOF
                    return 0;
                }
                // 待機登録
                if let Some(tid) = crate::task::current_thread_id() {
                    pb.set_waiter(tid.as_u64());
                }
            } else {
                return 0; // パイプが消えた → EOF
            }
        }
        // ロック解放後にスリープ
        if let Some(tid) = crate::task::current_thread_id() {
            crate::task::sleep_thread_unless_woken(tid);
            crate::task::yield_now();
        }

        // ウェイターをクリアしてリトライ
        {
            let table = PIPE_TABLE.lock();
            if let Some(Some(pb)) = table.get(id) {
                pb.clear_waiter();
            }
        }
    }
}

/// pipe(2) システムコール: pipefd[0]=読み込み端, pipefd[1]=書き込み端
pub fn pipe_syscall(pipefd_ptr: u64) -> u64 {
    pipe2_syscall(pipefd_ptr, 0)
}

/// pipe2(2) システムコール（flags: O_CLOEXEC / O_NONBLOCK に部分対応）
pub fn pipe2_syscall(pipefd_ptr: u64, flags: u64) -> u64 {
    use super::types::EFAULT;
    use crate::task::fd_table::{FileHandle, O_CLOEXEC};

    const EMFILE_VAL: u64 = (-24i64) as u64;

    if pipefd_ptr == 0 || !crate::syscall::validate_user_ptr(pipefd_ptr, 8) {
        return EFAULT;
    }

    let pipe_id = match alloc_pipe() {
        Some(id) => id,
        None => return EMFILE_VAL,
    };
    let cloexec = (flags & O_CLOEXEC) != 0;

    let pid = match crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id().as_u64()))
    {
        Some(p) => p,
        None => {
            close_read_end(pipe_id);
            close_write_end(pipe_id);
            return EFAULT;
        }
    };

    let read_handle = alloc::boxed::Box::new(FileHandle {
        data: alloc::boxed::Box::new([]),
        pos: 0,
        dir_path: None,
        is_remote: false,
        fd_remote: 0,
        remote_refs: None,
        pipe_id: Some(pipe_id),
        pipe_write: false,
        open_flags: 0,
    });
    let write_handle = alloc::boxed::Box::new(FileHandle {
        data: alloc::boxed::Box::new([]),
        pos: 0,
        dir_path: None,
        is_remote: false,
        fd_remote: 0,
        remote_refs: None,
        pipe_id: Some(pipe_id),
        pipe_write: true,
        open_flags: 1,
    });

    let pid_id = crate::task::ids::ProcessId::from_u64(pid);
    let read_fd =
        crate::task::with_process_mut(pid_id, |p| p.fd_table_mut().alloc(read_handle, cloexec))
            .flatten();
    let write_fd =
        crate::task::with_process_mut(pid_id, |p| p.fd_table_mut().alloc(write_handle, cloexec))
            .flatten();

    match (read_fd, write_fd) {
        (Some(rfd), Some(wfd)) => {
            let mut fds = [0u8; 8];
            fds[..4].copy_from_slice(&(rfd as u32).to_ne_bytes());
            fds[4..].copy_from_slice(&(wfd as u32).to_ne_bytes());
            crate::syscall::copy_to_user(pipefd_ptr, &fds)
                .map(|_| super::types::SUCCESS)
                .unwrap_or_else(|e| e)
        }
        _ => {
            close_read_end(pipe_id);
            close_write_end(pipe_id);
            EMFILE_VAL
        }
    }
}
