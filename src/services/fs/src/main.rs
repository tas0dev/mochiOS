use core::mem::size_of;
use std::boxed;
use swiftlib::ipc;
use swiftlib::task;

mod common;
mod disk_device;
mod ext2;
mod initfs;

use common::{resolve_path, FileHandle, FileSystem, VfsError};
use disk_device::DiskServiceDevice;
use ext2::Ext2Fs;
use initfs::InitFs;

const MAX_HANDLES: usize = 16;

#[derive(Clone, Copy)]
struct OpenFile {
    used: bool,
    handle: FileHandle,
    fs_id: usize,
}

impl OpenFile {
    const fn new() -> Self {
        Self {
            used: false,
            handle: FileHandle {
                inode: 0,
                offset: 0,
                flags: 0,
            },
            fs_id: 0,
        }
    }
}

static mut HANDLES: [OpenFile; MAX_HANDLES] = [OpenFile::new(); MAX_HANDLES];

/// マウントされたファイルシステム（ext2 優先、InitFs フォールバック）
static mut MOUNTED_FS: Option<Box<dyn FileSystem>> = None;

/// READY通知
const OP_NOTIFY_READY: u64 = 0xFF;

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

//noinspection ALL
/// disk.service から ext2 をマウントする（失敗時は InitFs にフォールバック）
fn mount_filesystem() {
    println!("[FS] mount_filesystem: searching for disk.service...");
    // disk.service を探す（最大5秒待つ）
    let mut disk_pid: Option<u64> = None;
    for i in 0..50 {
        if let Some(pid) = task::find_process_by_name("disk.service") {
            println!("[FS] Found disk.service at iteration {} PID={}", i, pid);
            disk_pid = Some(pid);
            break;
        }
        task::sleep(100);
    }

    if let Some(pid) = disk_pid {
        println!("[FS] Mounting ext2 from disk 1 via PID={}...", pid);
        let device = DiskServiceDevice::new(pid, 1); // disk 1 = Primary Slave = mochiOS.img
        println!("[FS] Calling Ext2Fs::new...");
        match Ext2Fs::new(Box::new(device)) {
            Ok(fs) => {
                println!("[FS] ext2 filesystem mounted from ATA disk.");
                unsafe {
                    MOUNTED_FS = Some(Box::new(fs));
                }
                return;
            }
            Err(e) => {
                println!("[FS] ext2 mount failed: {:?}, falling back to InitFs", e);
            }
        }
    } else {
        println!("[FS] disk.service not found, falling back to InitFs");
    }

    println!("[FS] Initializing InitFs...");
    // フォールバック: InitFs
    let mut initfs = InitFs::new();
    if let Err(e) = initfs.create_sample_files() {
        println!("[FS] Warning: Failed to create sample files: {:?}", e);
    }
    unsafe {
        MOUNTED_FS = Some(boxed::Box::new(initfs));
    }
    println!("[FS] InitFS mounted as fallback.");
}

/// core.service に準備完了を通知する
fn notify_ready_to_core() {
    let core_pid = match task::find_process_by_name("core.service") {
        Some(pid) => pid,
        None => {
            println!("[FS] WARNING: core.service not found, skipping READY notify");
            return;
        }
    };

    let op_bytes = OP_NOTIFY_READY.to_le_bytes();
    if ipc::ipc_send(core_pid, &op_bytes) == 0 {
        println!("[FS] Sent READY to core.service (PID={})", core_pid);
    }
}

fn main() {
    println!("[FS] Service Started.");

    mount_filesystem();
    notify_ready_to_core();

    let mut recv_buf = AlignedBuffer([0u8; 256]);

    loop {
        let (sender, len) = ipc::ipc_recv(&mut recv_buf.0);

        // EAGAIN (メッセージなし) の場合はCPUを譲る
        if sender == 0xFFFFFFFF || len == 0xFFFFFFFD {
            task::yield_now();
            continue;
        }

        if sender != 0 && (len as usize) >= size_of::<FsRequest>() {
            let req: FsRequest = unsafe { core::ptr::read(recv_buf.0.as_ptr() as *const _) };
            println!("[FS] REQ op={} from PID={}", req.op, sender);

            let mut resp = FsResponse {
                status: -1,
                len: 0,
                data: [0; 128],
            };

            match req.op {
                FsRequest::OP_OPEN => {
                    let mut path_len = 0;
                    while path_len < 128 && req.path[path_len] != 0 {
                        path_len += 1;
                    }

                    if let Ok(path_str) = core::str::from_utf8(&req.path[..path_len]) {
                        unsafe {
                            if let Some(ref fs) = MOUNTED_FS {
                                match resolve_path(fs.as_ref(), path_str) {
                                    Ok(inode) => {
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
                }
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
                }
                FsRequest::OP_WRITE => {
                    resp.status = vfs_error_to_errno(VfsError::NotSupported);
                }
                FsRequest::OP_CLOSE => {
                    let fd = req.arg1 as usize;
                    if fd < MAX_HANDLES && unsafe { HANDLES[fd].used } {
                        unsafe {
                            HANDLES[fd].used = false;
                        }
                        resp.status = 0;
                    } else {
                        resp.status = -9; // EBADF
                    }
                }
                _ => {
                    println!("[FS] Unknown OP: {}", req.op);
                    continue;
                }
            }

            let resp_slice = unsafe {
                core::slice::from_raw_parts(&resp as *const _ as *const u8, size_of::<FsResponse>())
            };

            let _ = ipc::ipc_send(sender, resp_slice);
        }
    }
}
