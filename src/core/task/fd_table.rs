//! プロセスごとのファイルディスクリプタテーブル

use alloc::boxed::Box;
use alloc::string::String;

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
    /// ファイル内容（initfs からロード済み）
    pub data: Box<[u8]>,
    /// 現在の読み取り/書き込み位置
    pub pos: usize,
    /// Some(path) であればディレクトリ fd
    pub dir_path: Option<String>,
}

/// プロセスごとのファイルディスクリプタテーブル
///
/// エントリは `Box<FileHandle>` の生ポインタ（0 = 未使用）。
/// サイズが大きいため必ず `Box<FdTable>` として使用すること。
pub struct FdTable {
    /// FD ごとの FileHandle 生ポインタ (0 = 空き)
    entries: [u64; PROCESS_MAX_FDS],
    /// FD ごとのフラグ (FD_CLOEXEC など)
    flags: [u8; PROCESS_MAX_FDS],
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
        unsafe { drop(Box::from_raw(ptr as *mut FileHandle)); }
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
        if ptr == 0 { None } else { Some(ptr as *mut FileHandle) }
    }

    /// FD の所有権を取り出す（close に相当）。
    pub fn take(&mut self, fd: usize) -> Option<Box<FileHandle>> {
        if fd < FD_BASE || fd >= PROCESS_MAX_FDS {
            return None;
        }
        let ptr = self.entries[fd];
        if ptr == 0 { return None; }
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
                unsafe { drop(Box::from_raw(ptr as *mut FileHandle)); }
            }
        }
    }

    /// すべての FD を閉じる（Drop で自動的に呼ばれる）。
    pub fn close_all(&mut self) {
        for i in FD_BASE..PROCESS_MAX_FDS {
            if self.entries[i] != 0 {
                let ptr = self.entries[i];
                self.entries[i] = 0;
                unsafe { drop(Box::from_raw(ptr as *mut FileHandle)); }
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
            if ptr == 0 { continue; }
            let fh = unsafe { &*(ptr as *const FileHandle) };
            let new_fh = Box::new(FileHandle {
                data: fh.data.clone(),
                pos: fh.pos,
                dir_path: fh.dir_path.clone(),
            });
            new_table.entries[i] = Box::into_raw(new_fh) as u64;
            new_table.flags[i] = self.flags[i];
        }
        new_table
    }

    /// FD のフラグを取得する。FD が未使用の場合 `None`。
    pub fn get_flags(&self, fd: usize) -> Option<u8> {
        if fd < FD_BASE || fd >= PROCESS_MAX_FDS { return None; }
        if self.entries[fd] == 0 { return None; }
        Some(self.flags[fd])
    }

    /// FD のフラグを設定する。FD が有効な場合 `true`。
    pub fn set_flags(&mut self, fd: usize, flags: u8) -> bool {
        if fd < FD_BASE || fd >= PROCESS_MAX_FDS { return false; }
        if self.entries[fd] == 0 { return false; }
        self.flags[fd] = flags;
        true
    }
}

impl Drop for FdTable {
    fn drop(&mut self) {
        self.close_all();
    }
}
