use core::mem::size_of;
use std::boxed;
use swiftlib::ipc;
use swiftlib::process;
use swiftlib::task;

mod common;
mod disk_device;
mod ext2;
mod initfs;

use common::vfs::FileType;
use common::{resolve_path, FileHandle, FileSystem, VfsError};
use disk_device::DiskServiceDevice;
use ext2::Ext2Fs;
use initfs::InitFs;

const MAX_HANDLES: usize = 16;
use swiftlib::fs_consts::{FS_DATA_MAX, FS_PATH_MAX, IPC_MAX_MSG_SIZE};
const READ_CACHE_SIZE: usize = 65536;
const ELF_HEADER_SIZE: usize = 64;
const ELF_PHDR_SIZE: usize = 56;
const ELF_PT_LOAD: u32 = 1;
const MAX_EXEC_IMAGE_SIZE: usize = 64 * 1024 * 1024;
// IPC_MAX_MSG_SIZE is defined in shared/fs_consts.rs; remove local duplicate
// pub(crate) const IPC_MAX_MSG_SIZE: usize = 2064;
const PENDING_IPC_CAPACITY: usize = 16;
const EXEC_CACHE_MAX_ENTRIES: usize = 8;
const EXEC_CACHE_MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
const EXEC_CACHE_MAX_IMAGE_SIZE: usize = 8 * 1024 * 1024;

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
    const OP_STAT: u64 = 6;
    const OP_FSTAT: u64 = 7;
    const OP_READDIR: u64 = 8;
    const OP_EXEC_STREAM: u64 = 9;
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FsResponse {
    status: i64,
    len: u64,
    data: [u8; FS_DATA_MAX],
}

#[repr(align(8))]
struct AlignedBuffer([u8; IPC_MAX_MSG_SIZE]);

#[inline]
fn decode_message_op(data: &[u8], len: usize) -> Option<u64> {
    if len < 8 || data.len() < 8 {
        return None;
    }
    Some(u64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]))
}

#[inline]
fn is_fs_request_message(data: &[u8], len: usize) -> bool {
    if len < size_of::<FsRequest>() {
        return false;
    }
    match decode_message_op(data, len) {
        Some(op) => {
            matches!(
                op,
                FsRequest::OP_OPEN
                    | FsRequest::OP_READ
                    | FsRequest::OP_WRITE
                    | FsRequest::OP_CLOSE
                    | FsRequest::OP_EXEC
                    | FsRequest::OP_STAT
                    | FsRequest::OP_FSTAT
                    | FsRequest::OP_READDIR
            )
        }
        None => false,
    }
}

#[derive(Clone, Copy)]
struct PendingIpcMessage {
    used: bool,
    sender: u64,
    len: usize,
    data: [u8; IPC_MAX_MSG_SIZE],
}

impl PendingIpcMessage {
    const fn new() -> Self {
        Self {
            used: false,
            sender: 0,
            len: 0,
            data: [0; IPC_MAX_MSG_SIZE],
        }
    }
}

static mut PENDING_IPC_MESSAGES: [PendingIpcMessage; PENDING_IPC_CAPACITY] =
    [PendingIpcMessage::new(); PENDING_IPC_CAPACITY];

pub(crate) fn enqueue_pending_message(sender: u64, data: &[u8]) -> bool {
    let copy_len = core::cmp::min(data.len(), IPC_MAX_MSG_SIZE);
    if copy_len == 0 {
        return true;
    }
    unsafe {
        for slot in &mut PENDING_IPC_MESSAGES {
            if !slot.used {
                slot.used = true;
                slot.sender = sender;
                slot.len = copy_len;
                slot.data[..copy_len].copy_from_slice(&data[..copy_len]);
                return true;
            }
        }
    }
    false
}

pub(crate) fn take_pending_message_for_sender(sender: u64, buf: &mut [u8]) -> Option<usize> {
    unsafe {
        for slot in &mut PENDING_IPC_MESSAGES {
            if slot.used && slot.sender == sender {
                let copy_len = core::cmp::min(slot.len, buf.len());
                buf[..copy_len].copy_from_slice(&slot.data[..copy_len]);
                slot.used = false;
                slot.sender = 0;
                slot.len = 0;
                return Some(copy_len);
            }
        }
    }
    None
}

pub(crate) fn take_pending_fs_request(
    disk_sender: Option<u64>,
    buf: &mut [u8],
) -> Option<(u64, usize)> {
    unsafe {
        for slot in &mut PENDING_IPC_MESSAGES {
            if !slot.used {
                continue;
            }
            if matches!(disk_sender, Some(disk_tid) if slot.sender == disk_tid) {
                continue;
            }
            if !is_fs_request_message(&slot.data, slot.len) {
                continue;
            }
            let copy_len = core::cmp::min(slot.len, buf.len());
            buf[..copy_len].copy_from_slice(&slot.data[..copy_len]);
            let sender = slot.sender;
            slot.used = false;
            slot.sender = 0;
            slot.len = 0;
            return Some((sender, copy_len));
        }
    }
    None
}

struct ExecCacheEntry {
    path: String,
    image: Vec<u8>,
    last_used: u64,
}

struct ExecImageCache {
    entries: Vec<ExecCacheEntry>,
    total_bytes: usize,
    use_counter: u64,
}

impl ExecImageCache {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            total_bytes: 0,
            use_counter: 0,
        }
    }

    fn next_use_counter(&mut self) -> u64 {
        self.use_counter = self.use_counter.wrapping_add(1);
        if self.use_counter == 0 {
            self.use_counter = 1;
        }
        self.use_counter
    }

    fn evict_lru(&mut self) {
        if self.entries.is_empty() {
            return;
        }

        let mut lru_index = 0usize;
        let mut lru_used = self.entries[0].last_used;
        for i in 1..self.entries.len() {
            if self.entries[i].last_used < lru_used {
                lru_index = i;
                lru_used = self.entries[i].last_used;
            }
        }

        let removed = self.entries.swap_remove(lru_index);
        self.total_bytes = self.total_bytes.saturating_sub(removed.image.len());
    }

    fn get(&mut self, path: &str) -> Option<&[u8]> {
        let idx = self.entries.iter().position(|entry| entry.path == path)?;
        let now = self.next_use_counter();
        self.entries[idx].last_used = now;
        Some(self.entries[idx].image.as_slice())
    }

    fn insert(&mut self, path: &str, image: Vec<u8>) {
        let image_len = image.len();
        if image_len == 0
            || image_len > EXEC_CACHE_MAX_IMAGE_SIZE
            || image_len > EXEC_CACHE_MAX_TOTAL_BYTES
        {
            return;
        }

        if let Some(idx) = self.entries.iter().position(|entry| entry.path == path) {
            let old = self.entries.swap_remove(idx);
            self.total_bytes = self.total_bytes.saturating_sub(old.image.len());
        }

        while self.entries.len() >= EXEC_CACHE_MAX_ENTRIES {
            self.evict_lru();
        }
        while self.total_bytes.saturating_add(image_len) > EXEC_CACHE_MAX_TOTAL_BYTES {
            if self.entries.is_empty() {
                return;
            }
            self.evict_lru();
        }

        let last_used = self.next_use_counter();
        self.total_bytes = self.total_bytes.saturating_add(image_len);
        self.entries.push(ExecCacheEntry {
            path: path.to_string(),
            image,
            last_used,
        });
    }
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

fn file_type_mode_bits(file_type: FileType) -> u16 {
    match file_type {
        FileType::RegularFile => 0x8000,
        FileType::Directory => 0x4000,
        FileType::SymbolicLink => 0xA000,
        FileType::BlockDevice => 0x6000,
        FileType::CharDevice => 0x2000,
        FileType::Fifo => 0x1000,
        FileType::Socket => 0xC000,
    }
}

fn mode_from_attr(file_type: FileType, mode: u16) -> u16 {
    let mut out = mode;
    if (out & 0xF000) == 0 {
        out |= file_type_mode_bits(file_type);
    }
    if (out & 0o777) == 0 {
        out |= 0o755;
    }
    out
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
        let n = fs.read(inode, offset + done as u64, &mut buf[done..])?;
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
            .checked_add(
                i.checked_mul(e_phentsize)
                    .ok_or(VfsError::InvalidArgument)?,
            )
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

fn decode_exec_path_and_args(raw: &[u8; 128]) -> Result<(String, Vec<String>), i64> {
    let mut path_end = 0usize;
    while path_end < raw.len() && raw[path_end] != 0 {
        path_end += 1;
    }
    if path_end == 0 {
        return Err(-22); // EINVAL
    }
    let path = core::str::from_utf8(&raw[..path_end])
        .map_err(|_| -22)?
        .to_string();

    let mut args = Vec::new();
    let mut i = path_end + 1;
    while i < raw.len() {
        if raw[i] == 0 {
            break;
        }
        let start = i;
        while i < raw.len() && raw[i] != 0 {
            i += 1;
        }
        let arg = core::str::from_utf8(&raw[start..i]).map_err(|_| -22)?;
        if !arg.is_empty() {
            args.push(arg.to_string());
        }
        if i < raw.len() {
            i += 1;
        }
    }
    Ok((path, args))
}

fn decode_path(raw: &[u8; 128]) -> Result<&str, i64> {
    let mut path_end = 0usize;
    while path_end < raw.len() && raw[path_end] != 0 {
        path_end += 1;
    }
    if path_end == 0 {
        return Err(-22); // EINVAL
    }
    core::str::from_utf8(&raw[..path_end]).map_err(|_| -22)
}

fn exec_from_image(
    path: &str,
    image: &[u8],
    args: &[String],
    requester_tid: u64,
) -> Result<u64, i64> {
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    // Prefer mapped, zero-copy streaming path via kernel (exec_from_fs_stream).
    match process::exec_via_fs_stream(path, &arg_refs) {
        Ok(pid) => Ok(pid),
        Err(_e) => {
            // Fallback to older copy-into-kernel path
            process::exec_from_buffer_named_with_args_and_requester(
                path,
                image,
                &arg_refs,
                requester_tid,
            )
        }
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
    let mut exec_cache = ExecImageCache::new();
    notify_ready_to_core();

    let mut recv_buf = Box::new(AlignedBuffer([0u8; IPC_MAX_MSG_SIZE]));

    loop {
        let disk_tid = task::find_process_by_name("disk.service");
        let (sender, len) = match take_pending_fs_request(disk_tid, &mut recv_buf.0) {
            Some((sender, len)) => (sender, len),
            None => {
                let (sender, len) = ipc::ipc_recv_wait(&mut recv_buf.0);
                if sender == 0 && len == 0 {
                    continue;
                }
                (sender, len as usize)
            }
        };

        // メッセージなし（エラー等で (0,0) が返る場合）
        if sender == 0 && len == 0 {
            continue;
        }

        // disk.service からのメッセージは、FsRequest 形式でないものを
        // disk_device 側待ち受け用の保留キューへ退避する。
        if let Some(disk_tid) = disk_tid {
            if sender == disk_tid {
                if !is_fs_request_message(&recv_buf.0, len) {
                    let msg_len = core::cmp::min(len, recv_buf.0.len());
                    if !enqueue_pending_message(sender, &recv_buf.0[..msg_len]) {
                        println!(
                            "[FS] WARN: pending IPC queue full (sender={}, len={})",
                            sender, len
                        );
                    }
                    continue;
                }
            }
        }

        if sender != 0 && len >= size_of::<FsRequest>() {
            let req: FsRequest =
                unsafe { core::ptr::read_unaligned(recv_buf.0.as_ptr() as *const _) };
            if req.op != FsRequest::OP_READ
                && req.op != FsRequest::OP_STAT
                && req.op != FsRequest::OP_FSTAT
                && req.op != FsRequest::OP_READDIR
            {
                println!("[FS] REQ op={} from PID={}", req.op, sender);
            }

            let mut resp = Box::new(FsResponse {
                status: -1,
                len: 0,
                data: [0; FS_DATA_MAX],
            });

            match req.op {
                FsRequest::OP_OPEN => {
                    let path_str = match decode_path(&req.path) {
                        Ok(v) => v,
                        Err(e) => {
                            resp.status = e;
                            let resp_slice = unsafe {
                                core::slice::from_raw_parts(
                                    resp.as_ref() as *const _ as *const u8,
                                    size_of::<FsResponse>(),
                                )
                            };
                            let _ = ipc::ipc_send(sender, resp_slice);
                            continue;
                        }
                    };
                    println!("[FS-DBG] OP_OPEN path='{}' from {}", path_str, sender);
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
                        } else {
                            resp.status = -5; // EIO
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
                                    let cache_end =
                                        open_file.cache_start + open_file.cache_len as u64;
                                    let cache_hit = open_file.cache_len > 0
                                        && offset >= open_file.cache_start
                                        && offset < cache_end;

                                    if !cache_hit {
                                        let cache_base = offset - (offset % READ_CACHE_SIZE as u64);
                                        match fs.read(inode, cache_base, &mut open_file.cache_data)
                                        {
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
                                        let cache_offset =
                                            (offset - open_file.cache_start) as usize;
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
                FsRequest::OP_STAT => {
                    let path_str = match decode_path(&req.path) {
                        Ok(v) => v,
                        Err(e) => {
                            resp.status = e;
                            let resp_slice = unsafe {
                                core::slice::from_raw_parts(
                                    resp.as_ref() as *const _ as *const u8,
                                    size_of::<FsResponse>(),
                                )
                            };
                            let _ = ipc::ipc_send(sender, resp_slice);
                            continue;
                        }
                    };
                    unsafe {
                        if let Some(ref fs) = MOUNTED_FS {
                            match resolve_path(fs.as_ref(), path_str)
                                .and_then(|inode| fs.stat(inode))
                            {
                                Ok(attr) => {
                                    resp.status = mode_from_attr(attr.file_type, attr.mode) as i64;
                                    resp.len = attr.size;
                                }
                                Err(e) => {
                                    resp.status = vfs_error_to_errno(e);
                                }
                            }
                        } else {
                            resp.status = -5; // EIO
                        }
                    }
                }
                FsRequest::OP_FSTAT => {
                    let fd = req.arg1 as usize;
                    if fd < MAX_HANDLES && unsafe { HANDLES[fd].used } {
                        unsafe {
                            if let Some(ref fs) = MOUNTED_FS {
                                let inode = HANDLES[fd].handle.inode;
                                match fs.stat(inode) {
                                    Ok(attr) => {
                                        resp.status =
                                            mode_from_attr(attr.file_type, attr.mode) as i64;
                                        resp.len = attr.size;
                                    }
                                    Err(e) => {
                                        resp.status = vfs_error_to_errno(e);
                                    }
                                }
                            } else {
                                resp.status = -5; // EIO
                            }
                        }
                    } else {
                        resp.status = -9; // EBADF
                    }
                }
                FsRequest::OP_READDIR => {
                    let fd = req.arg1 as usize;
                    if fd >= MAX_HANDLES || unsafe { !HANDLES[fd].used } {
                        resp.status = -9; // EBADF
                        continue;
                    }
                    let start = (req.arg2 >> 32) as usize;
                    let max_len = (req.arg2 & 0xFFFF_FFFF) as usize;
                    let max_len = core::cmp::min(max_len, FS_DATA_MAX);
                    if max_len == 0 {
                        resp.status = start as i64;
                        resp.len = 0;
                        continue;
                    }
                    unsafe {
                        if let Some(ref fs) = MOUNTED_FS {
                            let inode = HANDLES[fd].handle.inode;
                            match fs.readdir(inode) {
                                Ok(entries) => {
                                    let mut offset = 0usize;
                                    let mut next_index = start;
                                    for entry in entries.iter().skip(start) {
                                        if entry.name == "." || entry.name == ".." {
                                            next_index += 1;
                                            continue;
                                        }
                                        let name_bytes = entry.name.as_bytes();
                                        let need = name_bytes.len() + 1; // '\n'
                                        if need > max_len.saturating_sub(offset) {
                                            break;
                                        }
                                        resp.data[offset..offset + name_bytes.len()]
                                            .copy_from_slice(name_bytes);
                                        offset += name_bytes.len();
                                        resp.data[offset] = b'\n';
                                        offset += 1;
                                        next_index += 1;
                                    }
                                    resp.status = next_index as i64;
                                    resp.len = offset as u64;
                                }
                                Err(e) => {
                                    resp.status = vfs_error_to_errno(e);
                                }
                            }
                        } else {
                            resp.status = -5; // EIO
                        }
                    }
                }
                FsRequest::OP_EXEC => {
                    let (path_owned, args_owned) = match decode_exec_path_and_args(&req.path) {
                        Ok(v) => v,
                        Err(e) => {
                            resp.status = e;
                            let resp_slice = unsafe {
                                core::slice::from_raw_parts(
                                    resp.as_ref() as *const _ as *const u8,
                                    size_of::<FsResponse>(),
                                )
                            };
                            let _ = ipc::ipc_send(sender, resp_slice);
                            continue;
                        }
                    };
                    let path_str = path_owned.as_str();
                    let exec_ret = if let Some(image) = exec_cache.get(path_str) {
                        exec_from_image(path_str, image, &args_owned, sender)
                    } else {
                        unsafe {
                            if let Some(ref fs) = MOUNTED_FS {
                                match resolve_path(fs.as_ref(), path_str) {
                                    Ok(inode) => {
                                        match read_exec_image_from_inode(fs.as_ref(), inode) {
                                            Ok(elf_data) => {
                                                // Avoid invoking exec_via_fs_stream from fs.service itself
                                                // because the syscall path communicates back to fs.service
                                                // and may deadlock. Use the buffer-copy syscall that
                                                // accepts requester TID instead.
                                                let arg_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
                                                match process::exec_from_buffer_named_with_args_and_requester(
                                                    path_str,
                                                    &elf_data,
                                                    &arg_refs,
                                                    sender,
                                                ) {
                                                    Ok(pid) => {
                                                        exec_cache.insert(path_str, elf_data);
                                                        Ok(pid)
                                                    }
                                                    Err(errno) => Err(errno),
                                                }
                                            }
                                            Err(e) => Err(vfs_error_to_errno(e)),
                                        }
                                    }
                                    Err(e) => Err(vfs_error_to_errno(e)),
                                }
                            } else {
                                Err(-5) // EIO
                            }
                        }
                    };
                    match exec_ret {
                        Ok(pid) => resp.status = pid as i64,
                        Err(errno) => resp.status = errno,
                    }
                }
                FsRequest::OP_EXEC_STREAM => unsafe {
                    // Stream the ELF image back to requester in large chunks.
                    let (path_owned, _args_owned) = match decode_exec_path_and_args(&req.path) {
                        Ok(v) => v,
                        Err(e) => {
                            resp.status = e;
                            // send single error response
                            let resp_slice = unsafe {
                                core::slice::from_raw_parts(
                                    resp.as_ref() as *const _ as *const u8,
                                    size_of::<FsResponse>(),
                                )
                            };
                            let _ = ipc::ipc_send(sender, resp_slice);
                            continue;
                        }
                    };
                    let path_str = path_owned.as_str();
                    // Resolve inode and determine required_end (ELF validation) up-front so either streaming or mapped-write can use it
                    let inode = match unsafe { MOUNTED_FS.as_ref() } {
                        Some(fs_box) => match resolve_path(fs_box.as_ref(), path_str) {
                            Ok(i) => i,
                            Err(e) => unsafe {
                                resp.status = vfs_error_to_errno(e);
                                let resp_slice = core::slice::from_raw_parts(
                                    resp.as_ref() as *const _ as *const u8,
                                    size_of::<FsResponse>(),
                                );
                                let _ = ipc::ipc_send(sender, resp_slice);
                                continue;
                            },
                        },
                        None => unsafe {
                            resp.status = -5;
                            let resp_slice = core::slice::from_raw_parts(
                                resp.as_ref() as *const _ as *const u8,
                                size_of::<FsResponse>(),
                            );
                            let _ = ipc::ipc_send(sender, resp_slice);
                            continue;
                        },
                    };

                    // read ELF header and program headers to determine required_end
                    let mut ehdr = [0u8; ELF_HEADER_SIZE];
                    if let Err(e) =
                        read_exact_at(MOUNTED_FS.as_ref().unwrap().as_ref(), inode, 0, &mut ehdr)
                    {
                        resp.status = vfs_error_to_errno(e);
                        let resp_slice = core::slice::from_raw_parts(
                            resp.as_ref() as *const _ as *const u8,
                            size_of::<FsResponse>(),
                        );
                        let _ = ipc::ipc_send(sender, resp_slice);
                        continue;
                    }
                    if &ehdr[0..4] != b"\x7fELF" {
                        resp.status = -22;
                        let resp_slice = core::slice::from_raw_parts(
                            resp.as_ref() as *const _ as *const u8,
                            size_of::<FsResponse>(),
                        );
                        let _ = ipc::ipc_send(sender, resp_slice);
                        continue;
                    }

                    let e_phoff = read_u64_le(&ehdr, 32)
                        .and_then(|v| usize::try_from(v).ok())
                        .unwrap_or(0);
                    let e_phentsize = read_u16_le(&ehdr, 54).map(|v| v as usize).unwrap_or(0);
                    let e_phnum = read_u16_le(&ehdr, 56).map(|v| v as usize).unwrap_or(0);

                    if e_phnum == 0 || e_phentsize < ELF_PHDR_SIZE {
                        resp.status = -22;
                        let resp_slice = core::slice::from_raw_parts(
                            resp.as_ref() as *const _ as *const u8,
                            size_of::<FsResponse>(),
                        );
                        let _ = ipc::ipc_send(sender, resp_slice);
                        continue;
                    }

                    let ph_table_size = e_phentsize.checked_mul(e_phnum).unwrap_or(0);
                    let ph_end = e_phoff.checked_add(ph_table_size).unwrap_or(0);

                    // read hdr+ph table
                    let mut hdr_and_ph = vec![0u8; ph_end];
                    if let Err(e) = read_exact_at(
                        MOUNTED_FS.as_ref().unwrap().as_ref(),
                        inode,
                        0,
                        &mut hdr_and_ph,
                    ) {
                        resp.status = vfs_error_to_errno(e);
                        let resp_slice = core::slice::from_raw_parts(
                            resp.as_ref() as *const _ as *const u8,
                            size_of::<FsResponse>(),
                        );
                        let _ = ipc::ipc_send(sender, resp_slice);
                        continue;
                    }

                    let mut required_end = ph_end;
                    let mut has_load = false;
                    for i in 0..e_phnum {
                        let ph_off = e_phoff
                            .checked_add(i.checked_mul(e_phentsize).unwrap_or(0))
                            .unwrap_or(0);
                        let p_type = read_u32_le(&hdr_and_ph, ph_off).unwrap_or(0);
                        if p_type != ELF_PT_LOAD {
                            continue;
                        }
                        has_load = true;
                        let p_offset = read_u64_le(&hdr_and_ph, ph_off + 8).unwrap_or(0);
                        let p_filesz = read_u64_le(&hdr_and_ph, ph_off + 32).unwrap_or(0);
                        let seg_end_u64 = p_offset.checked_add(p_filesz).unwrap_or(0);
                        let seg_end = usize::try_from(seg_end_u64).unwrap_or(usize::MAX);
                        required_end = core::cmp::max(required_end, seg_end);
                    }

                    if !has_load || required_end == 0 || required_end > MAX_EXEC_IMAGE_SIZE {
                        resp.status = -22;
                        let resp_slice = core::slice::from_raw_parts(
                            resp.as_ref() as *const _ as *const u8,
                            size_of::<FsResponse>(),
                        );
                        let _ = ipc::ipc_send(sender, resp_slice);
                        continue;
                    }

                    // ストリーミングモードのみサポート（ゼロコピーは無効化）
                    // send initial FsResponse with total length
                    resp.status = 0;
                    resp.len = required_end as u64;
                    // send header
                    let resp_slice = core::slice::from_raw_parts(
                            resp.as_ref() as *const _ as *const u8,
                            size_of::<FsResponse>(),
                        );
                        let _ = ipc::ipc_send(sender, resp_slice);
                        // stream raw data chunks by reading from fs in chunks
                        let mut offset = 0usize;
                        let chunk_payload = IPC_MAX_MSG_SIZE;
                        unsafe {
                            let fs_ref: &dyn common::vfs::FileSystem =
                                &*MOUNTED_FS.as_ref().unwrap().as_ref();
                            while offset < required_end {
                                let take = core::cmp::min(chunk_payload, required_end - offset);
                                let mut tmp = vec![0u8; take];
                                match fs_ref.read(inode, offset as u64, &mut tmp) {
                                    Ok(nread) => {
                                        if nread == 0 {
                                            break;
                                        }
                                        let _ = ipc::ipc_send(sender, &tmp[..nread]);
                                        offset += nread;
                                    }
                                    Err(e) => {
                                        let err_resp = FsResponse {
                                            status: vfs_error_to_errno(e),
                                            len: 0,
                                            data: [0; FS_DATA_MAX],
                                        };
                                        let err_slice = core::slice::from_raw_parts(
                                            &err_resp as *const _ as *const u8,
                                            size_of::<FsResponse>(),
                                        );
                                        let _ = ipc::ipc_send(sender, err_slice);
                                        break;
                                    }
                                }
                            }
                        }
                        // done
                        continue;
                },
                0_u64 | 10_u64..=u64::MAX => todo!(),
            }

            // 通常の応答を送信（continue していない操作用）
            let resp_slice = unsafe {
                core::slice::from_raw_parts(
                    resp.as_ref() as *const _ as *const u8,
                    size_of::<FsResponse>(),
                )
            };
            let _ = ipc::ipc_send(sender, resp_slice);
        }
    }
}
