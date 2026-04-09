#![no_std]
#![no_main]

use core::arch::asm;
use core::cmp::min;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

const ATA_DATA: u16 = 0x1F0;
const ATA_SECTOR_COUNT: u16 = 0x1F2;
const ATA_LBA_LOW: u16 = 0x1F3;
const ATA_LBA_MID: u16 = 0x1F4;
const ATA_LBA_HIGH: u16 = 0x1F5;
const ATA_DRIVE_HEAD: u16 = 0x1F6;
const ATA_STATUS_CMD: u16 = 0x1F7;
const ATA_ALT_STATUS: u16 = 0x3F6;

const ATA_CMD_READ_SECTORS: u8 = 0x20;
const ATA_STATUS_ERR: u8 = 1 << 0;
const ATA_STATUS_DRQ: u8 = 1 << 3;
const ATA_STATUS_DF: u8 = 1 << 5;
const ATA_STATUS_BSY: u8 = 1 << 7;

const EXT2_MAGIC: u16 = 0xEF53;
const BLOCK_CACHE_SLOTS: usize = 64;
const INODE_CACHE_SLOTS: usize = 128;
const PATH_CACHE_SLOTS: usize = 128;
const PATH_CACHE_MAX: usize = 192;

#[repr(C)]
pub struct McxBuffer {
    pub ptr: *mut u8,
    pub len: usize,
}

#[repr(C)]
pub struct McxPath {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
pub struct McxFsOps {
    pub mount: extern "C" fn(device_id: u32) -> i32,
    pub read: extern "C" fn(path: McxPath, offset: u64, buf: McxBuffer, out_read: *mut usize) -> i32,
    pub stat: extern "C" fn(path: McxPath, out_mode: *mut u16, out_size: *mut u64) -> i32,
    pub readdir: extern "C" fn(path: McxPath, buf: McxBuffer, out_len: *mut usize) -> i32,
}

#[derive(Clone, Copy)]
struct FsMount {
    drive: u8, // 0=master, 1=slave
    block_size: u32,
    sectors_per_block: u32,
    inode_size: u16,
    inodes_per_group: u32,
    gdt_block: u32,
}

#[derive(Clone, Copy)]
struct BlockCacheEntry {
    valid: bool,
    drive: u8,
    block_num: u32,
    data: [u8; 4096],
}

impl BlockCacheEntry {
    const fn empty() -> Self {
        Self {
            valid: false,
            drive: 0,
            block_num: 0,
            data: [0u8; 4096],
        }
    }
}

#[derive(Clone, Copy)]
struct InodeCacheEntry {
    valid: bool,
    drive: u8,
    inode_num: u32,
    inode: [u8; 256],
}

impl InodeCacheEntry {
    const fn empty() -> Self {
        Self {
            valid: false,
            drive: 0,
            inode_num: 0,
            inode: [0u8; 256],
        }
    }
}

#[derive(Clone, Copy)]
struct PathCacheEntry {
    valid: bool,
    drive: u8,
    path_len: u16,
    path_hash: u64,
    inode_num: u32,
    path: [u8; PATH_CACHE_MAX],
}

impl PathCacheEntry {
    const fn empty() -> Self {
        Self {
            valid: false,
            drive: 0,
            path_len: 0,
            path_hash: 0,
            inode_num: 0,
            path: [0u8; PATH_CACHE_MAX],
        }
    }
}

static mut MOUNT: Option<FsMount> = None;
static OP_LOCK: AtomicBool = AtomicBool::new(false);

struct SharedBuf(UnsafeCell<[u8; 4096]>);

unsafe impl Sync for SharedBuf {}

impl SharedBuf {
    const fn new() -> Self {
        Self(UnsafeCell::new([0u8; 4096]))
    }

    #[inline]
    unsafe fn as_mut(&self) -> &mut [u8; 4096] {
        &mut *self.0.get()
    }

    #[inline]
    unsafe fn as_ref(&self) -> &[u8; 4096] {
        &*self.0.get()
    }
}

static READ_INODE_GDT_BLK: SharedBuf = SharedBuf::new();
static READ_INODE_IBLK: SharedBuf = SharedBuf::new();
static LOOKUP_BLK: SharedBuf = SharedBuf::new();
static LOOKUP_IND: SharedBuf = SharedBuf::new();
static READ_RANGE_BLK: SharedBuf = SharedBuf::new();
static READ_RANGE_IND: SharedBuf = SharedBuf::new();
static READDIR_BLK: SharedBuf = SharedBuf::new();
static READDIR_IND: SharedBuf = SharedBuf::new();

static mut BLOCK_CACHE: [BlockCacheEntry; BLOCK_CACHE_SLOTS] =
    [BlockCacheEntry::empty(); BLOCK_CACHE_SLOTS];
static mut BLOCK_CACHE_CURSOR: usize = 0;

static mut INODE_CACHE: [InodeCacheEntry; INODE_CACHE_SLOTS] =
    [InodeCacheEntry::empty(); INODE_CACHE_SLOTS];
static mut INODE_CACHE_CURSOR: usize = 0;

static mut PATH_CACHE: [PathCacheEntry; PATH_CACHE_SLOTS] = [PathCacheEntry::empty(); PATH_CACHE_SLOTS];
static mut PATH_CACHE_CURSOR: usize = 0;

struct OpLockGuard;

impl Drop for OpLockGuard {
    fn drop(&mut self) {
        OP_LOCK.store(false, Ordering::Release);
    }
}

#[inline]
fn lock_ops() -> OpLockGuard {
    while OP_LOCK
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    OpLockGuard
}

#[inline]
fn path_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

unsafe fn reset_caches() {
    for e in &mut BLOCK_CACHE {
        e.valid = false;
    }
    BLOCK_CACHE_CURSOR = 0;
    for e in &mut INODE_CACHE {
        e.valid = false;
    }
    INODE_CACHE_CURSOR = 0;
    for e in &mut PATH_CACHE {
        e.valid = false;
    }
    PATH_CACHE_CURSOR = 0;
}

unsafe fn block_cache_lookup(drive: u8, block_num: u32, out: &mut [u8], block_size: usize) -> bool {
    for e in &BLOCK_CACHE {
        if e.valid && e.drive == drive && e.block_num == block_num {
            out[..block_size].copy_from_slice(&e.data[..block_size]);
            return true;
        }
    }
    false
}

unsafe fn block_cache_insert(drive: u8, block_num: u32, data: &[u8], block_size: usize) {
    let slot = BLOCK_CACHE_CURSOR % BLOCK_CACHE_SLOTS;
    BLOCK_CACHE_CURSOR = (BLOCK_CACHE_CURSOR + 1) % BLOCK_CACHE_SLOTS;
    let ent = &mut BLOCK_CACHE[slot];
    ent.valid = true;
    ent.drive = drive;
    ent.block_num = block_num;
    ent.data[..block_size].copy_from_slice(&data[..block_size]);
}

unsafe fn inode_cache_lookup(drive: u8, inode_num: u32, out: &mut [u8; 256], isz: usize) -> bool {
    for e in &INODE_CACHE {
        if e.valid && e.drive == drive && e.inode_num == inode_num {
            out[..isz].copy_from_slice(&e.inode[..isz]);
            return true;
        }
    }
    false
}

unsafe fn inode_cache_insert(drive: u8, inode_num: u32, inode: &[u8; 256], isz: usize) {
    let slot = INODE_CACHE_CURSOR % INODE_CACHE_SLOTS;
    INODE_CACHE_CURSOR = (INODE_CACHE_CURSOR + 1) % INODE_CACHE_SLOTS;
    let ent = &mut INODE_CACHE[slot];
    ent.valid = true;
    ent.drive = drive;
    ent.inode_num = inode_num;
    ent.inode[..isz].copy_from_slice(&inode[..isz]);
}

unsafe fn path_cache_lookup(drive: u8, path: &[u8]) -> Option<u32> {
    if path.len() > PATH_CACHE_MAX {
        return None;
    }
    let h = path_hash(path);
    for e in &PATH_CACHE {
        if !e.valid || e.drive != drive || e.path_hash != h {
            continue;
        }
        let n = e.path_len as usize;
        if n == path.len() && e.path[..n] == path[..] {
            return Some(e.inode_num);
        }
    }
    None
}

unsafe fn path_cache_insert(drive: u8, path: &[u8], inode_num: u32) {
    if path.len() > PATH_CACHE_MAX {
        return;
    }
    let slot = PATH_CACHE_CURSOR % PATH_CACHE_SLOTS;
    PATH_CACHE_CURSOR = (PATH_CACHE_CURSOR + 1) % PATH_CACHE_SLOTS;
    let ent = &mut PATH_CACHE[slot];
    ent.valid = true;
    ent.drive = drive;
    ent.path_len = path.len() as u16;
    ent.path_hash = path_hash(path);
    ent.inode_num = inode_num;
    ent.path[..path.len()].copy_from_slice(path);
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let mut value: u8;
    asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    value
}

#[inline]
unsafe fn outb(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

#[inline]
unsafe fn inw(port: u16) -> u16 {
    let mut value: u16;
    asm!("in ax, dx", in("dx") port, out("ax") value, options(nomem, nostack, preserves_flags));
    value
}

#[inline]
unsafe fn io_wait_400ns() {
    let _ = inb(ATA_ALT_STATUS);
    let _ = inb(ATA_ALT_STATUS);
    let _ = inb(ATA_ALT_STATUS);
    let _ = inb(ATA_ALT_STATUS);
}

#[inline]
unsafe fn select_drive(drive: u8, lba: u32) {
    let head = 0xE0 | ((drive & 1) << 4) | (((lba >> 24) & 0x0F) as u8);
    outb(ATA_DRIVE_HEAD, head);
    io_wait_400ns();
}

unsafe fn wait_not_busy(timeout: usize) -> bool {
    for _ in 0..timeout {
        let st = inb(ATA_STATUS_CMD);
        if (st & ATA_STATUS_BSY) == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

unsafe fn wait_drq(timeout: usize) -> bool {
    for _ in 0..timeout {
        let st = inb(ATA_STATUS_CMD);
        if (st & ATA_STATUS_BSY) != 0 {
            core::hint::spin_loop();
            continue;
        }
        if (st & (ATA_STATUS_ERR | ATA_STATUS_DF)) != 0 {
            return false;
        }
        if (st & ATA_STATUS_DRQ) != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

unsafe fn read_sector_ata(drive: u8, lba: u32, out: &mut [u8; 512]) -> bool {
    if !wait_not_busy(200_000) {
        return false;
    }
    select_drive(drive, lba);
    outb(ATA_SECTOR_COUNT, 1);
    outb(ATA_LBA_LOW, (lba & 0xFF) as u8);
    outb(ATA_LBA_MID, ((lba >> 8) & 0xFF) as u8);
    outb(ATA_LBA_HIGH, ((lba >> 16) & 0xFF) as u8);
    outb(ATA_STATUS_CMD, ATA_CMD_READ_SECTORS);
    if !wait_drq(200_000) {
        return false;
    }
    for i in 0..256 {
        let w = inw(ATA_DATA);
        let b = w.to_le_bytes();
        out[i * 2] = b[0];
        out[i * 2 + 1] = b[1];
    }
    true
}

#[inline]
fn read_u16(buf: &[u8], off: usize) -> Option<u16> {
    let s = buf.get(off..off + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

#[inline]
fn read_u32(buf: &[u8], off: usize) -> Option<u32> {
    let s = buf.get(off..off + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

unsafe fn read_fs_block(m: &FsMount, block_num: u32, out: &mut [u8]) -> bool {
    let block_size = m.block_size as usize;
    if out.len() < block_size {
        return false;
    }
    if block_cache_lookup(m.drive, block_num, out, block_size) {
        return true;
    }
    let spb = m.sectors_per_block as usize;
    for i in 0..spb {
        let lba = block_num
            .saturating_mul(m.sectors_per_block)
            .saturating_add(i as u32);
        let mut sec = [0u8; 512];
        if !read_sector_ata(m.drive, lba, &mut sec) {
            return false;
        }
        let dst = i * 512;
        out[dst..dst + 512].copy_from_slice(&sec);
    }
    block_cache_insert(m.drive, block_num, out, block_size);
    true
}

unsafe fn probe_ext2_drive(drive: u8) -> Option<FsMount> {
    let mut s2 = [0u8; 512];
    let mut s3 = [0u8; 512];
    if !read_sector_ata(drive, 2, &mut s2) || !read_sector_ata(drive, 3, &mut s3) {
        return None;
    }
    let mut sb = [0u8; 1024];
    sb[..512].copy_from_slice(&s2);
    sb[512..].copy_from_slice(&s3);
    if read_u16(&sb, 56)? != EXT2_MAGIC {
        return None;
    }
    let log_block_size = read_u32(&sb, 24)?;
    if log_block_size > 2 {
        return None;
    }
    let block_size = 1024u32.checked_shl(log_block_size)?;
    if block_size < 1024 || block_size % 512 != 0 {
        return None;
    }
    let inode_size = read_u16(&sb, 88).unwrap_or(128);
    let inodes_per_group = read_u32(&sb, 40)?;
    if inodes_per_group == 0 {
        return None;
    }
    let gdt_block = if block_size == 1024 { 2 } else { 1 };
    Some(FsMount {
        drive,
        block_size,
        sectors_per_block: block_size / 512,
        inode_size,
        inodes_per_group,
        gdt_block,
    })
}

unsafe fn read_inode(m: &FsMount, inode_num: u32, inode_out: &mut [u8; 256]) -> bool {
    if inode_num == 0 || m.inodes_per_group == 0 {
        return false;
    }
    let isz = m.inode_size as usize;
    if inode_cache_lookup(m.drive, inode_num, inode_out, isz) {
        return true;
    }
    let group = (inode_num - 1) / m.inodes_per_group;
    let index = (inode_num - 1) % m.inodes_per_group;

    let gdt_entry_off = (group as usize) * 32;
    let gdt_block_off = gdt_entry_off / (m.block_size as usize);
    let gdt_inner = gdt_entry_off % (m.block_size as usize);
    if !read_fs_block(
        m,
        m.gdt_block + gdt_block_off as u32,
        READ_INODE_GDT_BLK.as_mut(),
    ) {
        return false;
    }
    let inode_table = match read_u32(READ_INODE_GDT_BLK.as_ref(), gdt_inner + 8) {
        Some(v) => v,
        None => return false,
    };
    let inode_off = (index as usize) * (m.inode_size as usize);
    let blk = inode_off / (m.block_size as usize);
    let off = inode_off % (m.block_size as usize);
    if !read_fs_block(m, inode_table + blk as u32, READ_INODE_IBLK.as_mut()) {
        return false;
    }
    let iblk = READ_INODE_IBLK.as_ref();
    if off + isz > iblk.len() || isz > inode_out.len() {
        return false;
    }
    inode_out[..isz].copy_from_slice(&iblk[off..off + isz]);
    inode_cache_insert(m.drive, inode_num, inode_out, isz);
    true
}

#[inline]
fn inode_mode(inode: &[u8]) -> u16 {
    read_u16(inode, 0).unwrap_or(0)
}

#[inline]
fn inode_size(inode: &[u8]) -> u32 {
    read_u32(inode, 4).unwrap_or(0)
}

#[inline]
fn inode_block(inode: &[u8], idx: usize) -> u32 {
    read_u32(inode, 40 + idx * 4).unwrap_or(0)
}

#[inline]
fn is_dir(mode: u16) -> bool {
    (mode & 0xF000) == 0x4000
}

unsafe fn read_data_block_num(
    m: &FsMount,
    inode: &[u8],
    block_idx: usize,
    scratch: &mut [u8; 4096],
) -> Option<u32> {
    if block_idx < 12 {
        let n = inode_block(inode, block_idx);
        return if n == 0 { None } else { Some(n) };
    }
    let idx = block_idx - 12;
    let per = (m.block_size / 4) as usize;
    if idx >= per {
        return None;
    }
    let indirect = inode_block(inode, 12);
    if indirect == 0 {
        return None;
    }
    if !read_fs_block(m, indirect, scratch) {
        return None;
    }
    let n = read_u32(scratch, idx * 4)?;
    if n == 0 { None } else { Some(n) }
}

unsafe fn lookup_child(m: &FsMount, dir_inode_num: u32, name: &[u8]) -> Option<u32> {
    let mut inode = [0u8; 256];
    if !read_inode(m, dir_inode_num, &mut inode) || !is_dir(inode_mode(&inode)) {
        return None;
    }
    let dir_size = inode_size(&inode) as usize;
    let block_size = m.block_size as usize;
    let blocks = dir_size.div_ceil(block_size);
    for bi in 0..blocks {
        let bnum = read_data_block_num(m, &inode, bi, LOOKUP_IND.as_mut())?;
        if !read_fs_block(m, bnum, LOOKUP_BLK.as_mut()) {
            return None;
        }
        let blk = LOOKUP_BLK.as_ref();
        let mut off = 0usize;
        while off + 8 <= block_size {
            let ino = read_u32(blk, off)?;
            let rec_len = read_u16(blk, off + 4)? as usize;
            let nlen = *blk.get(off + 6)? as usize;
            if rec_len == 0 || off + rec_len > block_size {
                break;
            }
            if ino != 0 && nlen > 0 && off + 8 + nlen <= block_size {
                let nm = &blk[off + 8..off + 8 + nlen];
                if nm == name {
                    return Some(ino);
                }
            }
            off += rec_len;
        }
    }
    None
}

unsafe fn resolve_path_inode(m: &FsMount, path: &[u8]) -> Option<u32> {
    if let Some(inode) = path_cache_lookup(m.drive, path) {
        return Some(inode);
    }
    let mut cur = 2u32;
    let mut i = 0usize;
    while i < path.len() {
        while i < path.len() && path[i] == b'/' {
            i += 1;
        }
        if i >= path.len() {
            break;
        }
        let start = i;
        while i < path.len() && path[i] != b'/' {
            i += 1;
        }
        let seg = &path[start..i];
        if seg.is_empty() || seg == b"." || seg == b".." {
            continue;
        }
        cur = lookup_child(m, cur, seg)?;
    }
    path_cache_insert(m.drive, path, cur);
    Some(cur)
}

unsafe fn read_inode_range(
    m: &FsMount,
    inode_num: u32,
    offset: u64,
    dst: &mut [u8],
) -> Option<usize> {
    let mut inode = [0u8; 256];
    if !read_inode(m, inode_num, &mut inode) {
        return None;
    }
    let size = inode_size(&inode) as u64;
    if offset >= size {
        return Some(0);
    }
    let to_read = min(dst.len() as u64, size - offset) as usize;
    let block_size = m.block_size as usize;
    let mut done = 0usize;
    while done < to_read {
        let file_off = offset as usize + done;
        let bi = file_off / block_size;
        let boff = file_off % block_size;
        let bnum = read_data_block_num(m, &inode, bi, READ_RANGE_IND.as_mut())?;
        if !read_fs_block(m, bnum, READ_RANGE_BLK.as_mut()) {
            return None;
        }
        let n = min(block_size - boff, to_read - done);
        let blk = READ_RANGE_BLK.as_ref();
        dst[done..done + n].copy_from_slice(&blk[boff..boff + n]);
        done += n;
    }
    Some(done)
}

unsafe fn read_path_inode(path: McxPath) -> Option<u32> {
    let m = MOUNT.as_ref()?;
    let raw = core::slice::from_raw_parts(path.ptr, path.len);
    let p = if !raw.is_empty() && raw[0] == b'/' {
        &raw[1..]
    } else {
        raw
    };
    resolve_path_inode(m, p)
}

extern "C" fn fs_mount(_device_id: u32) -> i32 {
    let _guard = lock_ops();
    unsafe {
        // rootfs は qemu-runner の disk0 (IDE index=1, primary slave) を優先。
        // 起動直後はデバイス準備に時間がかかるため複数回リトライする。
        for _ in 0..16 {
            if let Some(m) = probe_ext2_drive(1) {
                reset_caches();
                MOUNT = Some(m);
                return 0;
            }
            if let Some(m) = probe_ext2_drive(0) {
                reset_caches();
                MOUNT = Some(m);
                return 0;
            }
            for _ in 0..2_000_000 {
                core::hint::spin_loop();
            }
        }
    }
    -5
}

extern "C" fn fs_read(path: McxPath, offset: u64, buf: McxBuffer, out_read: *mut usize) -> i32 {
    if path.ptr.is_null() || buf.ptr.is_null() || out_read.is_null() {
        return -22;
    }
    let _guard = lock_ops();
    unsafe {
        let inode = match read_path_inode(path) {
            Some(v) => v,
            None => {
                return if MOUNT.is_some() { -2 } else { -5 };
            }
        };
        let m = match MOUNT.as_ref() {
            Some(v) => v,
            None => return -5,
        };
        let dst = core::slice::from_raw_parts_mut(buf.ptr, buf.len);
        match read_inode_range(m, inode, offset, dst) {
            Some(n) => {
                *out_read = n;
                0
            }
            None => -5,
        }
    }
}

extern "C" fn fs_stat(path: McxPath, out_mode: *mut u16, out_size: *mut u64) -> i32 {
    if path.ptr.is_null() || out_mode.is_null() || out_size.is_null() {
        return -22;
    }
    let _guard = lock_ops();
    unsafe {
        let inode_num = match read_path_inode(path) {
            Some(v) => v,
            None => {
                return if MOUNT.is_some() { -2 } else { -5 };
            }
        };
        let m = match MOUNT.as_ref() {
            Some(v) => v,
            None => return -5,
        };
        let mut inode = [0u8; 256];
        if !read_inode(m, inode_num, &mut inode) {
            return -5;
        }
        *out_mode = inode_mode(&inode);
        *out_size = inode_size(&inode) as u64;
        0
    }
}

extern "C" fn fs_readdir(path: McxPath, buf: McxBuffer, out_len: *mut usize) -> i32 {
    if path.ptr.is_null() || buf.ptr.is_null() || out_len.is_null() {
        return -22;
    }
    let _guard = lock_ops();
    unsafe {
        let inode_num = match read_path_inode(path) {
            Some(v) => v,
            None => {
                return if MOUNT.is_some() { -2 } else { -5 };
            }
        };
        let m = match MOUNT.as_ref() {
            Some(v) => v,
            None => return -5,
        };
        let mut inode = [0u8; 256];
        if !read_inode(m, inode_num, &mut inode) {
            return -5;
        }
        if !is_dir(inode_mode(&inode)) {
            return -20;
        }

        let block_size = m.block_size as usize;
        let dir_size = inode_size(&inode) as usize;
        let mut written = 0usize;
        let out = core::slice::from_raw_parts_mut(buf.ptr, buf.len);
        let blocks = dir_size.div_ceil(block_size);
        for bi in 0..blocks {
            let bnum = match read_data_block_num(m, &inode, bi, READDIR_IND.as_mut()) {
                Some(v) => v,
                None => return -5,
            };
            if !read_fs_block(m, bnum, READDIR_BLK.as_mut()) {
                return -5;
            }
            let data_blk = READDIR_BLK.as_ref();
            let mut off = 0usize;
            while off + 8 <= block_size {
                let ino = match read_u32(data_blk, off) {
                    Some(v) => v,
                    None => break,
                };
                let rec_len = match read_u16(data_blk, off + 4) {
                    Some(v) => v as usize,
                    None => break,
                };
                let nlen = match data_blk.get(off + 6) {
                    Some(v) => *v as usize,
                    None => break,
                };
                if rec_len == 0 || off + rec_len > block_size {
                    break;
                }
                if ino != 0 && nlen > 0 && off + 8 + nlen <= block_size {
                    let nm = &data_blk[off + 8..off + 8 + nlen];
                    if nm != b"." && nm != b".." {
                        let need = nlen + if written == 0 { 0 } else { 1 };
                        if written + need > out.len() {
                            *out_len = written;
                            return 0;
                        }
                        if written != 0 {
                            out[written] = b'\n';
                            written += 1;
                        }
                        out[written..written + nlen].copy_from_slice(nm);
                        written += nlen;
                    }
                }
                off += rec_len;
            }
        }
        *out_len = written;
        0
    }
}

static FS_OPS: McxFsOps = McxFsOps {
    mount: fs_mount,
    read: fs_read,
    stat: fs_stat,
    readdir: fs_readdir,
};

#[no_mangle]
pub extern "C" fn mochi_module_init() -> *const McxFsOps {
    &FS_OPS
}

#[used]
static KEEP_INIT_REF: extern "C" fn() -> *const McxFsOps = mochi_module_init;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
