#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::{self};
use core::mem::size_of;

use swiftlib::io;
use swiftlib::ipc;
use swiftlib::task;

mod common;
mod initfs;
mod ext2;

use common::{FileHandle, FileSystem, VfsError, resolve_path};
use initfs::InitFs;

const MAX_HANDLES: usize = 16;

#[derive(Clone, Copy)]
struct OpenFile {
    used: bool,
    handle: FileHandle,
    fs_id: usize,  // どのファイルシステムか
}

impl OpenFile {
    const fn new() -> Self {
        Self {
            used: false,
            handle: FileHandle { inode: 0, offset: 0, flags: 0 },
            fs_id: 0,
        }
    }
}

static mut HANDLES: [OpenFile; MAX_HANDLES] = [OpenFile::new(); MAX_HANDLES];

// マウントされたファイルシステム（簡易的に1つだけ）
static mut MOUNTED_FS: Option<InitFs> = None;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FsRequest {
    op: u64,
    arg1: u64,
    arg2: u64,
    path: [u8; 128],
}

impl FsRequest {
    const OP_OPEN: u64 = 1;
    const OP_READ: u64 = 2;
    const OP_WRITE: u64 = 3;
    const OP_CLOSE: u64 = 4;
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FsResponse {
    status: i64,
    len: u64,
    data: [u8; 128],
}

#[repr(align(8))]
struct AlignedBuffer([u8; 256]);

// 簡易的な標準出力ライター
struct Stdout;
impl fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        io::write_stdout(s.as_bytes());
        Ok(())
    }
}

macro_rules! print {
    ($($arg:tt)*) => ({
        let _ = core::fmt::Write::write_fmt(&mut Stdout, format_args!($($arg)*));
    });
}

macro_rules! println {
    () => (print!("\n"));
    ($($arg:tt)*) => (print!("{}\n", format_args!($($arg)*)));
}

fn vfs_error_to_errno(err: VfsError) -> i64 {
    match err {
        VfsError::NotFound => -2,          // ENOENT
        VfsError::PermissionDenied => -13, // EACCES
        VfsError::AlreadyExists => -17,    // EEXIST
        VfsError::IsDirectory => -21,      // EISDIR
        VfsError::NotDirectory => -20,     // ENOTDIR
        VfsError::InvalidArgument => -22,  // EINVAL
        VfsError::IoError => -5,           // EIO
        VfsError::OutOfSpace => -28,       // ENOSPC
        VfsError::ReadOnlyFs => -30,       // EROFS
        VfsError::TooManyOpenFiles => -24, // EMFILE
        VfsError::FileTooBig => -27,       // EFBIG
        VfsError::NotSupported => -38,     // ENOSYS
    }
}

#[no_mangle]
pub extern "C" fn main(_argc: i32, _argv: *const *const u8) -> i32 {
    println!("[FS] Service Started (VFS version).");

    // InitFSを初期化
    let mut initfs = InitFs::new();
    if let Err(e) = initfs.create_sample_files() {
        println!("[FS] Warning: Failed to create sample files: {:?}", e);
    }

    unsafe {
        MOUNTED_FS = Some(initfs);
    }

    println!("[FS] InitFS mounted and initialized.");

    let mut recv_buf = AlignedBuffer([0u8; 256]);

    loop {
        let (sender, len) = ipc::ipc_recv(&mut recv_buf.0);

        // EAGAIN (メッセージなし) の場合はCPUを譲る
        // EAGAIN時、sender=0xFFFFFFFF, len=0xFFFFFFFD になる
        if sender == 0xFFFFFFFF || len == 0xFFFFFFFD {
            task::yield_now();
            continue;
        }

        if sender != 0 && (len as usize) >= size_of::<FsRequest>() {
            let req: FsRequest = unsafe { core::ptr::read(recv_buf.0.as_ptr() as *const _) };
            println!("[FS] REQ op={} from PID={}", req.op, sender);

            let mut resp = FsResponse { status: -1, len: 0, data: [0; 128] };

            match req.op {
                FsRequest::OP_OPEN => {
                    // パスを文字列に変換
                    let mut path_len = 0;
                    while path_len < 128 && req.path[path_len] != 0 {
                        path_len += 1;
                    }
                    
                    if let Ok(path_str) = core::str::from_utf8(&req.path[..path_len]) {
                        unsafe {
                            if let Some(ref fs) = MOUNTED_FS {
                                match resolve_path(fs as &dyn FileSystem, path_str) {
                                    Ok(inode) => {
                                        // 空きハンドルを探す
                                        let mut handle_idx: i64 = -1;
                                        for i in 0..MAX_HANDLES {
                                            if !HANDLES[i].used {
                                                HANDLES[i].used = true;
                                                HANDLES[i].handle = FileHandle::new(inode, 0);
                                                HANDLES[i].fs_id = 0;
                                                handle_idx = i as i64;
                                                break;
                                            }
                                        }
                                        resp.status = handle_idx;
                                    }
                                    Err(e) => {
                                        resp.status = vfs_error_to_errno(e);
                                    }
                                }
                            }
                        }
                    }
                },
                FsRequest::OP_READ => {
                    let fd = req.arg1 as usize;
                    let read_len = req.arg2 as usize;

                    if fd < MAX_HANDLES && unsafe { HANDLES[fd].used } {
                        unsafe {
                            if let Some(ref fs) = MOUNTED_FS {
                                let handle = &mut HANDLES[fd].handle;
                                let inode = handle.inode;
                                let offset = handle.offset;

                                let mut buf = [0u8; 128];
                                let actual_len = core::cmp::min(read_len, 128);
                                
                                match fs.read(inode, offset, &mut buf[..actual_len]) {
                                    Ok(bytes_read) => {
                                        resp.data[..bytes_read].copy_from_slice(&buf[..bytes_read]);
                                        resp.len = bytes_read as u64;
                                        resp.status = bytes_read as i64;
                                        handle.offset += bytes_read as u64;
                                    }
                                    Err(e) => {
                                        resp.status = vfs_error_to_errno(e);
                                    }
                                }
                            }
                        }
                    } else {
                        resp.status = -9; // EBADF
                    }
                },
                FsRequest::OP_WRITE => {
                    // TODO: 書き込み実装
                    resp.status = vfs_error_to_errno(VfsError::NotSupported);
                },
                FsRequest::OP_CLOSE => {
                    let fd = req.arg1 as usize;
                    if fd < MAX_HANDLES && unsafe { HANDLES[fd].used } {
                        unsafe { HANDLES[fd].used = false; }
                        resp.status = 0;
                    } else {
                        resp.status = -9; // EBADF
                    }
                },
                _ => {
                    println!("[FS] Unknown OP: {}", req.op);
                }
            }

            let resp_slice = unsafe {
                core::slice::from_raw_parts(&resp as *const _ as *const u8, size_of::<FsResponse>())
            };

            let _ = ipc::ipc_send(sender, resp_slice);

        }
    }
}