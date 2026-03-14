//! 起動時にメモリへ展開済みのファイルシステム (read-only)
//!
//! - root 直下＋サブディレクトリ対応
//! - 直接ブロック + 単一間接ブロック対応
//! - 動的バッファで任意サイズのファイルを読み取り可能

use alloc::vec::Vec;
use core::str;

/// EXT2ファイルシステムのマジックナンバー
pub const EXT2_MAGIC: u16 = 0xEF53;

/// BootInfo から設定される initfs イメージへのスライス
/// ブートローダーが initfs_addr / initfs_size を設定した後に init() で初期化される
static mut INITFS_SLICE: &[u8] = &[];

/// rootfs (ext2) イメージへのスライス
static mut ROOTFS_SLICE: &[u8] = &[];

/// initfs スライスを BootInfo から設定する（kernel_entry から呼ばれる）
pub unsafe fn set_image(addr: u64, size: usize) {
    if addr != 0 && size != 0 {
        INITFS_SLICE = core::slice::from_raw_parts(addr as *const u8, size);
    }
}

/// rootfs スライスを BootInfo から設定する（kernel_entry から呼ばれる）
pub unsafe fn set_rootfs(addr: u64, size: usize) {
    if addr != 0 && size != 0 {
        ROOTFS_SLICE = core::slice::from_raw_parts(addr as *const u8, size);
    }
}

#[inline]
fn ext2_image() -> &'static [u8] {
    unsafe { INITFS_SLICE }
}

#[inline]
fn rootfs_image() -> &'static [u8] {
    unsafe { ROOTFS_SLICE }
}

/// スーパーブロックの構造体
#[derive(Debug, Clone, Copy)]
struct Superblock {
    /// ブロックサイズ (1024 << log_block_size)
    block_size: u32,
    /// inodeのサイズ
    inode_size: u16,
    /// グループあたりのinode数
    inodes_per_group: u32,
}

/// グループディスクリプタの構造体
#[derive(Debug, Clone, Copy)]
struct GroupDesc {
    /// inodeテーブルの開始ブロック番号
    inode_table: u32,
}

/// inodeの構造体
#[derive(Debug, Clone, Copy)]
struct Inode {
    /// ファイルの種類とアクセス権限
    mode: u16,
    /// ファイルサイズ
    size: u32,
    /// 直接ブロック + 単一間接ブロック + 二重間接ブロックのブロック番号
    blocks: [u32; 15],
}

/// ファイルシステムのエントリ（ファイル名とデータ）
#[derive(Debug, Clone)]
pub struct FsEntry<'a> {
    /// ファイル名
    pub name: &'a str,
    /// ファイルデータ
    pub data: Vec<u8>,
}

/// ファイルシステムのエントリを列挙するイテレータ
pub struct FsEntries<'a> {
    /// イメージ全体のバイトスライス
    image: &'a [u8],
    /// スーパーブロックの情報
    sb: Superblock,
    /// 対象ディレクトリのinode
    inode: Inode,
    /// 現在のブロックインデックス
    block_idx: usize,
    /// 現在のブロック内のオフセット
    offset: usize,
    /// ディレクトリ内の残りバイト数
    remaining_bytes: usize,
}

/// initfsを初期化して情報を出力する
pub fn init() {
    let sb = match superblock(ext2_image()) {
        Some(sb) => sb,
        None => {
            crate::warn!("initfs: invalid image");
            return;
        }
    };

    let root = match inode(ext2_image(), sb, 2) {
        Some(inode) if is_dir(inode.mode) => inode,
        _ => {
            crate::warn!("initfs: invalid root inode");
            return;
        }
    };

    crate::debug!(
        "initfs: block_size={} inode_size={}",
        sb.block_size,
        sb.inode_size
    );

    let mut count = 0usize;
    for entry in FsEntries::new(ext2_image(), sb, root) {
        crate::debug!("initfs: {} ({} bytes)", entry.name, entry.data.len());
        count += 1;
    }
    crate::debug!("initfs: {} entries", count);
}

/// ファイルを取得（rootfs を優先し、なければ initfs を検索）
///
/// ## Arguments
/// - `name`: ルートからのパス（例: "hello.txt", "System/fonts/ter-u12b.bdf"）
///
/// ## Returns
/// - ファイルが存在すれば内容のバイトベクタ、存在しなければNone
pub fn read(name: &str) -> Option<Vec<u8>> {
    // rootfs から先に検索
    if !rootfs_image().is_empty() {
        if let Some(data) = read_path_in(rootfs_image(), name) {
            return Some(data);
        }
    }
    // fallback: initfs
    read_path_in(ext2_image(), name)
}

/// ファイル一覧を取得（root直下）
///
/// ## Returns
/// - root直下のファイルとサブディレクトリを列挙するイテレータ
pub fn entries() -> FsEntries<'static> {
    let sb = superblock(ext2_image()).unwrap_or(Superblock {
        block_size: 1024,
        inode_size: 128,
        inodes_per_group: 0,
    });
    let root = inode(ext2_image(), sb, 2).unwrap_or(Inode {
        mode: 0,
        size: 0,
        blocks: [0; 15],
    });
    FsEntries::new(ext2_image(), sb, root)
}

/// 2バイトのリトルエンディアン整数を読み取る
fn read_u16(image: &[u8], offset: usize) -> Option<u16> {
    let bytes = image.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// 4バイトのリトルエンディアン整数を読み取る
fn read_u32(image: &[u8], offset: usize) -> Option<u32> {
    let bytes = image.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// スーパーブロックを読み取る
fn superblock(image: &[u8]) -> Option<Superblock> {
    if image.len() < 2048 {
        return None;
    }

    let sb_off = 1024;
    let magic = read_u16(image, sb_off + 56)?;

    if magic != EXT2_MAGIC {
        return None;
    }

    let log_block_size = read_u32(image, sb_off + 24)?;
    let block_size = 1024u32.checked_shl(log_block_size)?;
    let inode_size = read_u16(image, sb_off + 88)?;
    let inodes_per_group = read_u32(image, sb_off + 40)?;

    Some(Superblock {
        block_size,
        inode_size,
        inodes_per_group,
    })
}

fn group_desc(image: &[u8], sb: Superblock, group: u32) -> Option<GroupDesc> {
    let gdt_off = if sb.block_size == 1024 {
        (sb.block_size * 2) as usize
    } else {
        sb.block_size as usize
    };
    let desc_off = gdt_off + (group as usize) * 32;
    let inode_table = read_u32(image, desc_off + 8)?;
    Some(GroupDesc { inode_table })
}

fn inode(image: &[u8], sb: Superblock, inode_num: u32) -> Option<Inode> {
    if inode_num == 0 {
        return None;
    }
    // inodes_per_group が 0 なら除算パニックを防ぐ (#30)
    if sb.inodes_per_group == 0 {
        return None;
    }
    let group = (inode_num - 1) / sb.inodes_per_group;
    let index = (inode_num - 1) % sb.inodes_per_group;
    let gd = group_desc(image, sb, group)?;
    let inode_table = gd.inode_table as usize * sb.block_size as usize;
    let inode_off = inode_table + (index as usize) * (sb.inode_size as usize);

    let mode = read_u16(image, inode_off)?;
    let size = read_u32(image, inode_off + 4)?;

    let mut blocks = [0u32; 15];
    let blocks_off = inode_off + 40;
    for (i, block) in blocks.iter_mut().enumerate() {
        *block = read_u32(image, blocks_off + i * 4)?;
    }

    Some(Inode { mode, size, blocks })
}

fn is_dir(mode: u16) -> bool {
    mode & 0x4000 != 0
}

fn block_slice(image: &[u8], block_size: u32, block: u32) -> Option<&[u8]> {
    if block == 0 {
        return None;
    }
    let start = block as usize * block_size as usize;
    let end = start + block_size as usize;
    image.get(start..end)
}

fn data_block_number(
    image: &[u8],
    sb: Superblock,
    inode: Inode,
    block_index: usize,
) -> Option<u32> {
    // block_size が 0 なら除算パニックを防ぐ
    if sb.block_size == 0 {
        return None;
    }
    let entries_per_block = sb.block_size as usize / 4;

    // 直接ブロック (0-11)
    if block_index < 12 {
        return Some(inode.blocks[block_index]);
    }

    // 単一間接ブロック (12 .. 12+N)
    let idx = block_index - 12;
    if idx < entries_per_block {
        let indirect = inode.blocks[12];
        if indirect == 0 {
            return None;
        }
        let block = block_slice(image, sb.block_size, indirect)?;
        return read_u32(block, idx * 4);
    }

    // 二重間接ブロック (12+N .. 12+N+N*N)
    let idx2 = idx - entries_per_block;
    if idx2 < entries_per_block * entries_per_block {
        let dindirect = inode.blocks[13];
        if dindirect == 0 {
            return None;
        }
        let l1 = block_slice(image, sb.block_size, dindirect)?;
        let l1_idx = idx2 / entries_per_block;
        let l1_entry = read_u32(l1, l1_idx * 4)?;
        if l1_entry == 0 {
            return None;
        }
        let l2 = block_slice(image, sb.block_size, l1_entry)?;
        let l2_idx = idx2 % entries_per_block;
        return read_u32(l2, l2_idx * 4);
    }

    None
}

fn read_inode_data(image: &[u8], sb: Superblock, inode_num: u32) -> Option<Vec<u8>> {
    let inode = inode(image, sb, inode_num)?;
    if is_dir(inode.mode) {
        return Some(Vec::new());
    }
    if inode.size == 0 {
        return Some(Vec::new());
    }
    let size = inode.size as usize;
    let blocks_needed = size.div_ceil(sb.block_size as usize);
    let mut buf = Vec::with_capacity(size);

    for block_idx in 0..blocks_needed {
        let block_num = data_block_number(image, sb, inode, block_idx)?;
        if block_num == 0 {
            // スパースファイルのホール: ゼロで埋めて続行
            let to_fill = core::cmp::min(sb.block_size as usize, size - buf.len());
            buf.extend(core::iter::repeat_n(0u8, to_fill));
            if buf.len() >= size {
                break;
            }
            continue;
        }
        let block = block_slice(image, sb.block_size, block_num)?;
        let to_copy = core::cmp::min(block.len(), size - buf.len());
        buf.extend_from_slice(&block[..to_copy]);
        if buf.len() >= size {
            break;
        }
    }

    Some(buf)
}

impl<'a> FsEntries<'a> {
    fn new(image: &'a [u8], sb: Superblock, inode: Inode) -> Self {
        Self {
            image,
            sb,
            inode,
            block_idx: 0,
            offset: 0,
            remaining_bytes: inode.size as usize,
        }
    }
}

impl<'a> Iterator for FsEntries<'a> {
    type Item = FsEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.remaining_bytes == 0 {
                return None;
            }

            let block = data_block_number(self.image, self.sb, self.inode, self.block_idx)?;
            if block == 0 {
                return None;
            }
            let data = block_slice(self.image, self.sb.block_size, block)?;
            if self.offset >= data.len() {
                self.block_idx += 1;
                self.offset = 0;
                continue;
            }

            let base = self.offset;
            let inode = read_u32(data, base)?;
            let rec_len = read_u16(data, base + 4)? as usize;
            let name_len = *data.get(base + 6)? as usize;

            if rec_len == 0 {
                return None;
            }

            self.offset += rec_len;
            self.remaining_bytes = self.remaining_bytes.saturating_sub(rec_len);

            if inode == 0 {
                continue;
            }

            let name_bytes = data.get(base + 8..base + 8 + name_len)?;
            let name = str::from_utf8(name_bytes).ok()?;
            let data = read_inode_data(self.image, self.sb, inode).unwrap_or_default();
            return Some(FsEntry { name, data });
        }
    }
}

fn find_inode_in_dir(image: &[u8], sb: Superblock, dir_inode: Inode, name: &str) -> Option<u32> {
    if !is_dir(dir_inode.mode) {
        return None;
    }
    let mut block_idx = 0usize;
    let mut offset = 0usize;
    let mut remaining_bytes = dir_inode.size as usize;
    while remaining_bytes > 0 {
        let block = data_block_number(image, sb, dir_inode, block_idx)?;
        if block == 0 {
            return None;
        }
        let data = block_slice(image, sb.block_size, block)?;
        if offset >= data.len() {
            block_idx += 1;
            offset = 0;
            continue;
        }
        let base = offset;
        let inode_num = read_u32(data, base)?;
        let rec_len = read_u16(data, base + 4)? as usize;
        let name_len = *data.get(base + 6)? as usize;
        if rec_len == 0 {
            return None;
        }
        offset += rec_len;
        remaining_bytes = remaining_bytes.saturating_sub(rec_len);
        if inode_num == 0 {
            continue;
        }
        let name_bytes = data.get(base + 8..base + 8 + name_len)?;
        let entry_name = str::from_utf8(name_bytes).ok()?;
        if entry_name == name {
            return Some(inode_num);
        }
    }
    None
}

/// パスがディレクトリかどうかを確認する（rootfs優先、"."と"/"はルートとして扱う）
/// ファイルメタデータ: (inode_mode, size_bytes)
///
/// - `inode_mode` は ext2 の inode mode フィールド（ファイル種別 + パーミッション）
/// - ファイルが存在しない場合は `None`
pub fn file_metadata(path: &str) -> Option<(u16, u64)> {
    if !rootfs_image().is_empty() {
        if let Some(m) = file_metadata_in(rootfs_image(), path) {
            return Some(m);
        }
    }
    file_metadata_in(ext2_image(), path)
}

fn file_metadata_in(image: &[u8], path: &str) -> Option<(u16, u64)> {
    let sb = superblock(image)?;
    let normalized = path.trim_matches('/');
    if normalized.is_empty() || normalized == "." {
        // ルートディレクトリ
        let root = inode(image, sb, 2)?;
        return Some((root.mode, 0));
    }
    let mut current = inode(image, sb, 2)?;
    let mut parts = normalized.split('/').filter(|p| !p.is_empty()).peekable();
    while let Some(part) = parts.next() {
        if part == ".." || part == "." {
            continue;
        }
        let inode_num = find_inode_in_dir(image, sb, current, part)?;
        let next = inode(image, sb, inode_num)?;
        if parts.peek().is_none() {
            // 最終コンポーネント
            return Some((next.mode, next.size as u64));
        }
        current = next;
    }
    None
}

pub fn is_directory(path: &str) -> bool {
    if !rootfs_image().is_empty() && resolve_dir_inode_in(rootfs_image(), path).is_some() {
        return true;
    }
    resolve_dir_inode_in(ext2_image(), path).is_some()
}

/// ディレクトリのエントリ名一覧を返す（"."と".."は除く、rootfs優先）
pub fn readdir_path(path: &str) -> Option<alloc::vec::Vec<alloc::string::String>> {
    if !rootfs_image().is_empty() {
        if let Some(names) = readdir_path_in(rootfs_image(), path) {
            return Some(names);
        }
    }
    readdir_path_in(ext2_image(), path)
}

/// パスをディレクトリのinode番号に解決する（"."と"/"はルート inode 2を返す）
fn resolve_dir_inode_in(image: &[u8], path: &str) -> Option<u32> {
    let sb = superblock(image)?;
    let normalized = path.trim_matches('/');
    // "." や "" はルートを指す
    if normalized.is_empty() || normalized == "." {
        let root = inode(image, sb, 2)?;
        return if is_dir(root.mode) { Some(2) } else { None };
    }
    let mut current_num = 2u32;
    let mut current = inode(image, sb, current_num)?;
    for part in normalized.split('/').filter(|p| !p.is_empty()) {
        if part == "." {
            continue;
        }
        if part == ".." {
            // ".." はルートより上には行かない
            continue;
        }
        if !is_dir(current.mode) {
            return None;
        }
        let next_num = find_inode_in_dir(image, sb, current, part)?;
        current = inode(image, sb, next_num)?;
        current_num = next_num;
    }
    if is_dir(current.mode) { Some(current_num) } else { None }
}

fn readdir_path_in(image: &[u8], path: &str) -> Option<alloc::vec::Vec<alloc::string::String>> {
    let sb = superblock(image)?;
    let dir_inode_num = resolve_dir_inode_in(image, path)?;
    let dir_inode = inode(image, sb, dir_inode_num)?;

    let mut names = alloc::vec::Vec::new();
    let mut block_idx = 0usize;
    let mut offset = 0usize;
    let mut remaining_bytes = dir_inode.size as usize;

    while remaining_bytes > 0 {
        let block_num = data_block_number(image, sb, dir_inode, block_idx)?;
        if block_num == 0 {
            break;
        }
        let data = block_slice(image, sb.block_size, block_num)?;
        if offset >= data.len() {
            block_idx += 1;
            offset = 0;
            continue;
        }
        let base = offset;
        let entry_inode = read_u32(data, base)?;
        let rec_len = read_u16(data, base + 4)? as usize;
        let name_len = *data.get(base + 6)? as usize;
        if rec_len == 0 {
            break;
        }
        offset += rec_len;
        remaining_bytes = remaining_bytes.saturating_sub(rec_len);
        if entry_inode == 0 {
            continue;
        }
        let name_bytes = data.get(base + 8..base + 8 + name_len)?;
        let name = str::from_utf8(name_bytes).ok()?;
        if name != "." && name != ".." {
            names.push(alloc::string::String::from(name));
        }
    }
    Some(names)
}

fn read_path_in(image: &[u8], path: &str) -> Option<Vec<u8>> {
    let sb = superblock(image)?;
    let mut current = inode(image, sb, 2)?; // root

    let mut parts = path.split('/').filter(|p| !p.is_empty()).peekable();
    parts.peek()?;

    while let Some(part) = parts.next() {
        // ディレクトリトラバーサル防止: ".." および "." を拒否する (C-7修正)
        if part == ".." || part == "." {
            return None;
        }
        let is_last = parts.peek().is_none();
        let inode_num = find_inode_in_dir(image, sb, current, part)?;
        let next_inode = inode(image, sb, inode_num)?;
        if is_last {
            if is_dir(next_inode.mode) {
                return None;
            }
            return read_inode_data(image, sb, inode_num);
        }
        if !is_dir(next_inode.mode) {
            return None;
        }
        current = next_inode;
    }
    None
}
