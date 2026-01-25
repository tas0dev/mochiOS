//! 起動時にメモリへ展開済みのext2 (read-only)

use core::str;

const EXT2_MAGIC: u16 = 0xEF53;
const EXT2_IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/initfs.ext2"));

#[derive(Debug, Clone, Copy)]
struct Superblock {
	block_size: u32,
	inode_size: u16,
	inodes_per_group: u32,
}

#[derive(Debug, Clone, Copy)]
struct GroupDesc {
	inode_table: u32,
}

#[derive(Debug, Clone, Copy)]
struct Inode {
	mode: u16,
	size: u32,
	blocks: [u32; 15],
}

#[derive(Debug, Clone, Copy)]
pub struct FsEntry<'a> {
	pub name: &'a str,
	pub data: &'a [u8],
}

pub struct FsEntries<'a> {
	image: &'a [u8],
	sb: Superblock,
	inode: Inode,
	block_idx: usize,
	offset: usize,
	remaining_bytes: usize,
}

fn read_u16(image: &[u8], offset: usize) -> Option<u16> {
	let bytes = image.get(offset..offset + 2)?;
	Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(image: &[u8], offset: usize) -> Option<u32> {
	let bytes = image.get(offset..offset + 4)?;
	Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

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
	let group = (inode_num - 1) / sb.inodes_per_group;
	let index = (inode_num - 1) % sb.inodes_per_group;
	let gd = group_desc(image, sb, group)?;
	let inode_table = gd.inode_table as usize * sb.block_size as usize;
	let inode_off = inode_table + (index as usize) * (sb.inode_size as usize);

	let mode = read_u16(image, inode_off)?;
	let size = read_u32(image, inode_off + 4)?;

	let mut blocks = [0u32; 15];
	let blocks_off = inode_off + 40;
	for i in 0..15 {
		blocks[i] = read_u32(image, blocks_off + i * 4)?;
	}

	Some(Inode { mode, size, blocks })
}

fn is_dir(mode: u16) -> bool {
	mode & 0x4000 != 0
}

fn block_slice<'a>(image: &'a [u8], block_size: u32, block: u32) -> Option<&'a [u8]> {
	if block == 0 {
		return None;
	}
	let start = block as usize * block_size as usize;
	let end = start + block_size as usize;
	image.get(start..end)
}

fn data_block_number(image: &[u8], sb: Superblock, inode: Inode, block_index: usize) -> Option<u32> {
	if block_index < 12 {
		return Some(inode.blocks[block_index]);
	}
	let indirect = inode.blocks[12];
	if indirect == 0 {
		return None;
	}
	let entries_per_block = sb.block_size as usize / 4;
	let idx = block_index.checked_sub(12)?;
	if idx >= entries_per_block {
		return None;
	}
	let block = block_slice(image, sb.block_size, indirect)?;
	read_u32(block, idx * 4)
}

const READ_BUFFER_SIZE: usize = 4 * 1024 * 1024;
static mut READ_BUFFER: [u8; READ_BUFFER_SIZE] = [0; READ_BUFFER_SIZE];

fn read_inode_data(image: &[u8], sb: Superblock, inode_num: u32) -> Option<&'static [u8]> {
	let inode = inode(image, sb, inode_num)?;
	if is_dir(inode.mode) {
		return Some(&[]);
	}
	if inode.size == 0 {
		return Some(&[]);
	}
	let size = inode.size as usize;
	if size > READ_BUFFER_SIZE {
		return None;
	}
	let blocks_needed = (size + sb.block_size as usize - 1) / sb.block_size as usize;
	let mut written = 0usize;

	for block_idx in 0..blocks_needed {
		let block_num = data_block_number(image, sb, inode, block_idx)?;
		if block_num == 0 {
			return None;
		}
		let block = block_slice(image, sb.block_size, block_num)?;
		let to_copy = core::cmp::min(block.len(), size - written);
		unsafe {
			READ_BUFFER[written..written + to_copy].copy_from_slice(&block[..to_copy]);
		}
		written += to_copy;
		if written >= size {
			break;
		}
	}

	unsafe { Some(&READ_BUFFER[..size]) }
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
			let data = read_inode_data(self.image, self.sb, inode).unwrap_or(&[]);
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

fn read_path(path: &str) -> Option<&'static [u8]> {
	let sb = superblock(EXT2_IMAGE)?;
	let mut current = inode(EXT2_IMAGE, sb, 2)?; // root

	let mut parts = path.split('/').filter(|p| !p.is_empty()).peekable();
	if parts.peek().is_none() {
		return None;
	}

	while let Some(part) = parts.next() {
		let is_last = parts.peek().is_none();
		let inode_num = find_inode_in_dir(EXT2_IMAGE, sb, current, part)?;
		let next_inode = inode(EXT2_IMAGE, sb, inode_num)?;
		if is_last {
			if is_dir(next_inode.mode) {
				return None;
			}
			return read_inode_data(EXT2_IMAGE, sb, inode_num);
		}
		if !is_dir(next_inode.mode) {
			return None;
		}
		current = next_inode;
	}
	None
}

/// 初期FSを初期化して情報を出力
pub fn init() {
	let sb = match superblock(EXT2_IMAGE) {
		Some(sb) => sb,
		None => {
			crate::warn!("initfs(ext2): invalid image");
			return;
		}
	};

	let root = match inode(EXT2_IMAGE, sb, 2) {
		Some(inode) if is_dir(inode.mode) => inode,
		_ => {
			crate::warn!("initfs(ext2): invalid root inode");
			return;
		}
	};

	crate::info!("initfs(ext2): block_size={} inode_size={}", sb.block_size, sb.inode_size);

	let mut count = 0usize;
	for entry in FsEntries::new(EXT2_IMAGE, sb, root) {
		crate::debug!("initfs(ext2): {} ({} bytes)", entry.name, entry.data.len());
		count += 1;
	}
	crate::info!("initfs(ext2): {} entries", count);
}

/// ファイルを取得
pub fn read(name: &str) -> Option<&'static [u8]> {
	read_path(name)
}

/// ファイル一覧を取得（root直下）
pub fn entries() -> FsEntries<'static> {
	let sb = superblock(EXT2_IMAGE).unwrap_or(Superblock {
		block_size: 1024,
		inode_size: 128,
		inodes_per_group: 0,
	});
	let root = inode(EXT2_IMAGE, sb, 2).unwrap_or(Inode {
		mode: 0,
		size: 0,
		blocks: [0; 15],
	});
	FsEntries::new(EXT2_IMAGE, sb, root)
}
