//! プロセスごとのファイルディスクリプタテーブル

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

/// stdin / stdout / stderr の予約 FD 番号
pub const FD_BASE: usize = 3;

/// プロセスあたりの最大 FD 数
pub const PROCESS_MAX_FDS: usize = 256;

/// FD フラグ: exec 時にクローズする
pub const FD_CLOEXEC: u8 = 0x01;

/// open() フラグ: O_CLOEXEC (Linux: 0o2000000 = 0x80000)
pub const O_CLOEXEC: u64 = 0x80000;

/// オープンファイルの状態を保持するハンドル
pub struct FileHandle {
    /// ファイル内容（initfs からロード済み、パイプの場合は空）
    pub data: Box<[u8]>,
    /// 現在の読み取り/書き込み位置（パイプの場合はエントリインデックス兼用）
    pub pos: usize,
    /// Some(path) であればディレクトリ fd
    pub dir_path: Option<String>,
    /// true の場合、データはリモート FD バックエンドで管理される（fd_remote 値を参照）
    pub is_remote: bool,
    /// リモートバックエンド側のファイルディスクリプタ（is_remote=true のとき有効）
    pub fd_remote: u64,
    /// is_remote=true の場合の参照カウント（close時の二重クローズ防止）
    pub remote_refs: Option<Arc<AtomicUsize>>,
    /// Some(id) であればパイプ fd（グローバル PIPE_TABLE のインデックス）
    pub pipe_id: Option<usize>,
    /// パイプの書き込み端の場合 true
    pub pipe_write: bool,
    /// open()/openat() のファイル状態フラグ（F_GETFL/F_SETFL 用）
    pub open_flags: u64,
}

impl FileHandle {
    pub fn new_pipe_read(pipe_id: usize) -> Self {
        Self {
            data: Box::new([]),
            pos: 0,
            dir_path: None,
            is_remote: false,
            fd_remote: 0,
            remote_refs: None,
            pipe_id: Some(pipe_id),
            pipe_write: false,
            open_flags: 0,
        }
    }

    pub fn new_pipe_write(pipe_id: usize) -> Self {
        Self {
            data: Box::new([]),
            pos: 0,
            dir_path: None,
            is_remote: false,
            fd_remote: 0,
            remote_refs: None,
            pipe_id: Some(pipe_id),
            pipe_write: true,
            open_flags: 1,
        }
    }

    #[inline]
    pub fn clone_remote_refs(&self) -> Option<Arc<AtomicUsize>> {
        if !self.is_remote {
            return None;
        }
        self.remote_refs.as_ref().map(|refs| {
            refs.fetch_add(1, Ordering::AcqRel);
            refs.clone()
        })
    }
}

impl Drop for FileHandle {
    fn drop(&mut self) {
        if !self.is_remote {
            return;
        }
        if let Some(refs) = self.remote_refs.as_ref() {
            if refs.fetch_sub(1, Ordering::AcqRel) == 1 {
                crate::syscall::fs::close_remote_fd_from_kernel(self.fd_remote);
            }
        } else {
            crate::syscall::fs::close_remote_fd_from_kernel(self.fd_remote);
        }
    }
}

/// プロセスごとのファイルディスクリプタテーブル
///
/// エントリは `Box<FileHandle>` の生ポインタ（0 = 未使用）。
/// サイズが大きいため必ず `Box<FdTable>` として使用すること。
pub struct FdTable {
    /// FD ごとの FileHandle 生ポインタ (0 = 空き)
    pub(crate) entries: [u64; PROCESS_MAX_FDS],
    /// FD ごとのフラグ (FD_CLOEXEC など)
    pub(crate) flags: [u8; PROCESS_MAX_FDS],
}

impl FdTable {
    /// ヒープ上に FdTable をゼロ初期化して作成する。
    ///
    /// `Box::new(FdTable { ... })` はスタック上への一時配置を招くため、
    /// `alloc_zeroed` で直接ヒープに確保する。
    pub fn new_boxed() -> Box<Self> {
        unsafe {
            let layout = core::alloc::Layout::new::<Self>();
            let ptr = alloc::alloc::alloc_zeroed(layout) as *mut Self;
            Box::from_raw(ptr)
        }
    }

    /// 新しい FileHandle を割り当て、使用した FD 番号 (>= FD_BASE) を返す。
    ///
    /// 空きスロットがない場合は `None`。
    pub fn alloc(&mut self, handle: Box<FileHandle>, cloexec: bool) -> Option<usize> {
        let ptr = Box::into_raw(handle) as u64;
        for i in FD_BASE..PROCESS_MAX_FDS {
            if self.entries[i] == 0 {
                self.entries[i] = ptr;
                self.flags[i] = if cloexec { FD_CLOEXEC } else { 0 };
                return Some(i);
            }
        }
        // スロット不足: ハンドルを解放
        unsafe {
            drop(Box::from_raw(ptr as *mut FileHandle));
        }
        None
    }

    /// FD に対応する FileHandle の生ポインタを返す（所有権は移動しない）。
    ///
    /// # Safety
    /// 呼び出し元はポインタが有効な間に close_fd() を呼ばないことを保証すること。
    pub fn get_raw(&self, fd: usize) -> Option<*mut FileHandle> {
        if fd < FD_BASE || fd >= PROCESS_MAX_FDS {
            return None;
        }
        let ptr = self.entries[fd];
        if ptr == 0 {
            None
        } else {
            Some(ptr as *mut FileHandle)
        }
    }

    /// FD に対応する FileHandle の参照を返す。
    pub fn get(&self, fd: usize) -> Option<&FileHandle> {
        self.get_raw(fd).map(|ptr| unsafe { &*ptr })
    }

    /// FD に対応する FileHandle の可変参照を返す。
    pub fn get_mut(&mut self, fd: usize) -> Option<&mut FileHandle> {
        self.get_raw(fd).map(|ptr| unsafe { &mut *ptr })
    }

    /// FD の所有権を取り出す（close に相当）。
    pub fn take(&mut self, fd: usize) -> Option<Box<FileHandle>> {
        if fd < FD_BASE || fd >= PROCESS_MAX_FDS {
            return None;
        }
        let ptr = self.entries[fd];
        if ptr == 0 {
            return None;
        }
        self.entries[fd] = 0;
        self.flags[fd] = 0;
        Some(unsafe { Box::from_raw(ptr as *mut FileHandle) })
    }

    /// FD を閉じる。閉じた場合 `true`、既に空きの場合 `false`。
    pub fn close_fd(&mut self, fd: usize) -> bool {
        self.take(fd).is_some()
    }

    /// FD_CLOEXEC が設定されているすべての FD を閉じる（execve 時に呼ぶ）。
    pub fn close_cloexec_fds(&mut self) {
        for i in FD_BASE..PROCESS_MAX_FDS {
            if self.entries[i] != 0 && (self.flags[i] & FD_CLOEXEC) != 0 {
                let ptr = self.entries[i];
                self.entries[i] = 0;
                self.flags[i] = 0;
                unsafe {
                    drop(Box::from_raw(ptr as *mut FileHandle));
                }
            }
        }
    }

    /// すべての FD を閉じる（Drop で自動的に呼ばれる）。
    pub fn close_all(&mut self) {
        for i in FD_BASE..PROCESS_MAX_FDS {
            if self.entries[i] != 0 {
                let ptr = self.entries[i];
                self.entries[i] = 0;
                unsafe {
                    drop(Box::from_raw(ptr as *mut FileHandle));
                }
            }
        }
    }

    /// fork 用: 全エントリを複製して新しい FdTable を返す。
    ///
    /// 親子は独立したファイル位置を持つ（簡易コピーセマンティクス）。
    pub fn clone_for_fork(&self) -> Box<FdTable> {
        let mut new_table = FdTable::new_boxed();
        for i in FD_BASE..PROCESS_MAX_FDS {
            let ptr = self.entries[i];
            if ptr == 0 {
                continue;
            }
            let fh = unsafe { &*(ptr as *const FileHandle) };
            let new_fh = Box::new(FileHandle {
                data: fh.data.clone(),
                pos: fh.pos,
                dir_path: fh.dir_path.clone(),
                is_remote: fh.is_remote,
                fd_remote: fh.fd_remote,
                remote_refs: fh.clone_remote_refs(),
                pipe_id: fh.pipe_id,
                pipe_write: fh.pipe_write,
                open_flags: fh.open_flags,
            });
            new_table.entries[i] = Box::into_raw(new_fh) as u64;
            new_table.flags[i] = self.flags[i];
        }
        new_table
    }

    /// FD のフラグを取得する。FD が未使用の場合 `None`。
    pub fn get_flags(&self, fd: usize) -> Option<u8> {
        if fd < FD_BASE || fd >= PROCESS_MAX_FDS {
            return None;
        }
        if self.entries[fd] == 0 {
            return None;
        }
        Some(self.flags[fd])
    }

    /// FD のフラグを設定する。FD が有効な場合 `true`。
    pub fn set_flags(&mut self, fd: usize, flags: u8) -> bool {
        if fd < FD_BASE || fd >= PROCESS_MAX_FDS {
            return false;
        }
        if self.entries[fd] == 0 {
            return false;
        }
        self.flags[fd] = flags;
        true
    }
}

impl Drop for FdTable {
    fn drop(&mut self) {
        self.close_all();
    }
}
