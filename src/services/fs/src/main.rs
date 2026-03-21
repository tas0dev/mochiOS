use core::mem::size_of;
use std::boxed;
use swiftlib::ipc;
use swiftlib::process;
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
const FS_DATA_MAX: usize = 560;
const READ_CACHE_SIZE: usize = 4096;
const EXEC_READ_CHUNK: usize = 64 * 1024;
const ELF_HEADER_SIZE: usize = 64;
const ELF_PHDR_SIZE: usize = 56;
const ELF_PT_LOAD: u32 = 1;
const MAX_EXEC_IMAGE_SIZE: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy)]
struct OpenFile {
    used: bool,
    handle: FileHandle,
    fs_id: usize,
    cache_start: u64,
    cache_len: usize,
    cache_data: [u8; READ_CACHE_SIZE],
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
            cache_start: 0,
            cache_len: 0,
            cache_data: [0; READ_CACHE_SIZE],
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
    const OP_EXEC: u64 = 5;
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FsResponse {
    status: i64,
    len: u64,
    data: [u8; FS_DATA_MAX],
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

#[inline]
fn read_u16_le(buf: &[u8], offset: usize) -> Option<u16> {
    let bytes = buf.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

#[inline]
fn read_u32_le(buf: &[u8], offset: usize) -> Option<u32> {
    let bytes = buf.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[inline]
fn read_u64_le(buf: &[u8], offset: usize) -> Option<u64> {
    let bytes = buf.get(offset..offset + 8)?;
    Some(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn read_exact_at(
    fs: &dyn FileSystem,
    inode: u64,
    offset: u64,
    buf: &mut [u8],
) -> Result<(), VfsError> {
    let mut done = 0usize;
    while done < buf.len() {
        let end = core::cmp::min(done + EXEC_READ_CHUNK, buf.len());
        let n = fs.read(inode, offset + done as u64, &mut buf[done..end])?;
        if n == 0 {
            return Err(VfsError::IoError);
        }
        done += n;
    }
    Ok(())
}

fn read_exec_image_from_inode(fs: &dyn FileSystem, inode: u64) -> Result<Vec<u8>, VfsError> {
    let file_size_u64 = fs.stat(inode)?.size;
    let file_size = usize::try_from(file_size_u64).map_err(|_| VfsError::FileTooBig)?;
    if file_size < ELF_HEADER_SIZE {
        return Err(VfsError::InvalidArgument);
    }

    let mut ehdr = [0u8; ELF_HEADER_SIZE];
    read_exact_at(fs, inode, 0, &mut ehdr)?;
    if &ehdr[0..4] != b"\x7fELF" {
        return Err(VfsError::InvalidArgument);
    }

    let e_phoff = read_u64_le(&ehdr, 32)
        .and_then(|v| usize::try_from(v).ok())
        .ok_or(VfsError::InvalidArgument)?;
    let e_phentsize = read_u16_le(&ehdr, 54)
        .map(|v| v as usize)
        .ok_or(VfsError::InvalidArgument)?;
    let e_phnum = read_u16_le(&ehdr, 56)
        .map(|v| v as usize)
        .ok_or(VfsError::InvalidArgument)?;

    if e_phnum == 0 || e_phentsize < ELF_PHDR_SIZE {
        return Err(VfsError::InvalidArgument);
    }

    let ph_table_size = e_phentsize
        .checked_mul(e_phnum)
        .ok_or(VfsError::InvalidArgument)?;
    let ph_end = e_phoff
        .checked_add(ph_table_size)
        .ok_or(VfsError::InvalidArgument)?;
    if ph_end > file_size {
        return Err(VfsError::InvalidArgument);
    }

    let mut hdr_and_ph = vec![0u8; ph_end];
    read_exact_at(fs, inode, 0, &mut hdr_and_ph)?;

    let mut required_end = ph_end;
    let mut has_load = false;
    for i in 0..e_phnum {
        let ph_off = e_phoff
            .checked_add(i.checked_mul(e_phentsize).ok_or(VfsError::InvalidArgument)?)
            .ok_or(VfsError::InvalidArgument)?;
        if ph_off
            .checked_add(ELF_PHDR_SIZE)
            .map_or(true, |end| end > hdr_and_ph.len())
        {
            return Err(VfsError::InvalidArgument);
        }

        let p_type = read_u32_le(&hdr_and_ph, ph_off).ok_or(VfsError::InvalidArgument)?;
        if p_type != ELF_PT_LOAD {
            continue;
        }
        has_load = true;

        let p_offset = read_u64_le(&hdr_and_ph, ph_off + 8).ok_or(VfsError::InvalidArgument)?;
        let p_filesz = read_u64_le(&hdr_and_ph, ph_off + 32).ok_or(VfsError::InvalidArgument)?;
        let seg_end_u64 = p_offset
            .checked_add(p_filesz)
            .ok_or(VfsError::InvalidArgument)?;
        let seg_end = usize::try_from(seg_end_u64).map_err(|_| VfsError::FileTooBig)?;
        required_end = core::cmp::max(required_end, seg_end);
    }

    if !has_load {
        return Err(VfsError::InvalidArgument);
    }
    if required_end > file_size {
        return Err(VfsError::InvalidArgument);
    }
    if required_end > MAX_EXEC_IMAGE_SIZE {
        return Err(VfsError::FileTooBig);
    }

    let mut image = vec![0u8; required_end];
    image[..ph_end].copy_from_slice(&hdr_and_ph);
    if required_end > ph_end {
        read_exact_at(fs, inode, ph_end as u64, &mut image[ph_end..])?;
    }
    Ok(image)
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
        let (sender, len) = ipc::ipc_recv_wait(&mut recv_buf.0);

        // メッセージなし（エラー等で (0,0) が返る場合）
        if sender == 0 && len == 0 {
            continue;
        }

        if sender != 0 && (len as usize) >= size_of::<FsRequest>() {
            let req: FsRequest = unsafe { core::ptr::read_unaligned(recv_buf.0.as_ptr() as *const _) };
            if req.op != FsRequest::OP_READ {
                println!("[FS] REQ op={} from PID={}", req.op, sender);
            }

            let mut resp = FsResponse {
                status: -1,
                len: 0,
                data: [0; FS_DATA_MAX],
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
                                                HANDLES[i].cache_start = 0;
                                                HANDLES[i].cache_len = 0;
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
                                let open_file = &mut HANDLES[fd];
                                let handle = &mut open_file.handle;
                                let inode = handle.inode;
                                let offset = handle.offset;
                                let actual_len = core::cmp::min(read_len, FS_DATA_MAX);

                                if actual_len == 0 {
                                    resp.status = 0;
                                    resp.len = 0;
                                } else {
                                    let mut can_serve = true;
                                    let cache_end = open_file.cache_start + open_file.cache_len as u64;
                                    let cache_hit = open_file.cache_len > 0
                                        && offset >= open_file.cache_start
                                        && offset < cache_end;

                                    if !cache_hit {
                                        let cache_base =
                                            offset - (offset % READ_CACHE_SIZE as u64);
                                        match fs.read(inode, cache_base, &mut open_file.cache_data) {
                                            Ok(bytes_read) => {
                                                open_file.cache_start = cache_base;
                                                open_file.cache_len = bytes_read;
                                            }
                                            Err(e) => {
                                                resp.status = vfs_error_to_errno(e);
                                                can_serve = false;
                                            }
                                        }
                                    }

                                    if !can_serve {
                                        // resp.status はエラー設定済み
                                    } else if open_file.cache_len == 0 {
                                        // EOF
                                        resp.status = 0;
                                        resp.len = 0;
                                    } else {
                                        let cache_offset = (offset - open_file.cache_start) as usize;
                                        if cache_offset >= open_file.cache_len {
                                            resp.status = 0;
                                            resp.len = 0;
                                        } else {
                                            let bytes_read = core::cmp::min(
                                                actual_len,
                                                open_file.cache_len - cache_offset,
                                            );
                                            resp.data[..bytes_read].copy_from_slice(
                                                &open_file.cache_data
                                                    [cache_offset..cache_offset + bytes_read],
                                            );
                                            resp.len = bytes_read as u64;
                                            resp.status = bytes_read as i64;
                                            handle.offset += bytes_read as u64;
                                        }
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
                            HANDLES[fd].cache_start = 0;
                            HANDLES[fd].cache_len = 0;
                        }
                        resp.status = 0;
                    } else {
                        resp.status = -9; // EBADF
                    }
                }
                FsRequest::OP_EXEC => {
                    let mut path_len = 0;
                    while path_len < 128 && req.path[path_len] != 0 {
                        path_len += 1;
                    }

                    if let Ok(path_str) = core::str::from_utf8(&req.path[..path_len]) {
                        println!("[FS] OP_EXEC: {}", path_str);
                        unsafe {
                            if let Some(ref fs) = MOUNTED_FS {
                                match resolve_path(fs.as_ref(), path_str) {
                                    Ok(inode) => match read_exec_image_from_inode(fs.as_ref(), inode) {
                                        Ok(elf_data) => {
                                            match process::exec_from_buffer(&elf_data) {
                                                Ok(pid) => {
                                                    resp.status = pid as i64;
                                                }
                                                Err(_) => {
                                                    resp.status = -5; // EIO
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            resp.status = vfs_error_to_errno(e);
                                        }
                                    },
                                    Err(e) => {
                                        resp.status = vfs_error_to_errno(e);
                                    }
                                }
                            } else {
                                resp.status = -5; // EIO
                            }
                        }
                    } else {
                        resp.status = -22; // EINVAL
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

            let send_ret = ipc::ipc_send(sender, resp_slice);
            if send_ret != 0 {
                println!("[FS] WARN: failed to send response to {} (ret={})", sender, send_ret);
            }
        }
    }
}
