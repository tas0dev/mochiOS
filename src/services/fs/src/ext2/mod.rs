//! EXT2 ファイルシステム実装
//!
//! Linux標準のext2ファイルシステムをサポート

use std::boxed::Box;
use std::string::String;
use std::vec::Vec;

use crate::common::vfs::{
    DirEntry, FileAttr, FileSystem, FileType, VfsError, VfsResult,
};

/// ブロックデバイストレイト
///
/// 実際のストレージデバイスへのアクセスを抽象化
#[allow(unused)]
pub trait BlockDevice: Send + Sync {
    /// ブロックサイズ（通常512バイト）
    fn block_size(&self) -> usize;
    
    /// ブロックを読み取る
    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), ()>;
    
    /// ブロックに書き込む
    fn write_block(&mut self, block_num: u64, buf: &[u8]) -> Result<(), ()>;
}

/// EXT2スーパーブロック
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct Ext2Superblock {
    s_inodes_count: u32,        // inode総数
    s_blocks_count: u32,        // ブロック総数
    s_r_blocks_count: u32,      // 予約ブロック数
    s_free_blocks_count: u32,   // 空きブロック数
    s_free_inodes_count: u32,   // 空きinode数
    s_first_data_block: u32,    // 最初のデータブロック
    s_log_block_size: u32,      // ブロックサイズ (1024 << s_log_block_size)
    s_log_frag_size: u32,       // フラグメントサイズ
    s_blocks_per_group: u32,    // グループあたりブロック数
    s_frags_per_group: u32,     // グループあたりフラグメント数
    s_inodes_per_group: u32,    // グループあたりinode数
    s_mtime: u32,               // マウント時刻
    s_wtime: u32,               // 書き込み時刻
    s_mnt_count: u16,           // マウント回数
    s_max_mnt_count: u16,       // 最大マウント回数
    s_magic: u16,               // マジックナンバー (0xEF53)
    s_state: u16,               // ファイルシステム状態
    s_errors: u16,              // エラー時の動作
    s_minor_rev_level: u16,     // マイナーリビジョン
    s_lastcheck: u32,           // 最終チェック時刻
    s_checkinterval: u32,       // チェック間隔
    s_creator_os: u32,          // 作成OS
    s_rev_level: u32,           // リビジョンレベル
    s_def_resuid: u16,          // 予約ブロックのデフォルトUID
    s_def_resgid: u16,          // 予約ブロックのデフォルトGID
    // EXT2_DYNAMIC_REV (rev_level == 1) の追加フィールド
    s_first_ino: u32,           // 最初の使用可能inode
    s_inode_size: u16,          // inodeサイズ
    // ... その他のフィールドは省略
}

const EXT2_MAGIC: u16 = 0xEF53;
const EXT2_SUPERBLOCK_OFFSET: u64 = 1024;

/// EXT2 inode
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct Ext2Inode {
    i_mode: u16,                // ファイルモード
    i_uid: u16,                 // 所有者UID
    i_size: u32,                // サイズ（下位32ビット）
    i_atime: u32,               // アクセス時刻
    i_ctime: u32,               // 作成時刻
    i_mtime: u32,               // 変更時刻
    i_dtime: u32,               // 削除時刻
    i_gid: u16,                 // グループID
    i_links_count: u16,         // ハードリンク数
    i_blocks: u32,              // ブロック数
    i_flags: u32,               // フラグ
    i_osd1: u32,                // OS依存1
    i_block: [u32; 15],         // ブロックポインタ
    i_generation: u32,          // ファイルバージョン
    i_file_acl: u32,            // ファイルACL
    i_dir_acl: u32,             // ディレクトリACL
    i_faddr: u32,               // フラグメントアドレス
    i_osd2: [u8; 12],           // OS依存2
}

// inode モードフラグ
const EXT2_S_IFREG: u16 = 0x8000;   // 通常ファイル
const EXT2_S_IFDIR: u16 = 0x4000;   // ディレクトリ
const EXT2_S_IFLNK: u16 = 0xA000;   // シンボリックリンク

/// EXT2ディレクトリエントリ
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct Ext2DirEntry {
    inode: u32,         // inode番号
    rec_len: u16,       // このエントリのサイズ
    name_len: u8,       // 名前の長さ
    file_type: u8,      // ファイルタイプ
    // name: [u8]       // 可変長の名前（name_lenバイト）
}

/// ブロックグループディスクリプタ
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct Ext2GroupDesc {
    bg_block_bitmap: u32,       // ブロックビットマップのブロック番号
    bg_inode_bitmap: u32,       // inodeビットマップのブロック番号
    bg_inode_table: u32,        // inodeテーブルの開始ブロック番号
    bg_free_blocks_count: u16,  // 空きブロック数
    bg_free_inodes_count: u16,  // 空きinode数
    bg_used_dirs_count: u16,    // ディレクトリ数
    bg_pad: u16,                // パディング
    bg_reserved: [u32; 3],      // 予約
}

/// EXT2ファイルシステム
#[allow(dead_code)]
pub struct Ext2Fs {
    device: Box<dyn BlockDevice>,
    superblock: Ext2Superblock,
    block_size: usize,
    inodes_per_group: u32,
    blocks_per_group: u32,
    inode_size: usize,
    group_desc_table: Vec<Ext2GroupDesc>,
}

impl Ext2Fs {
    /// 新しいEXT2ファイルシステムを作成
    pub fn new(device: Box<dyn BlockDevice>) -> VfsResult<Self> {
        // スーパーブロックを読み取る
        let mut sb_buf = vec![0u8; 1024];
        device.read_block(EXT2_SUPERBLOCK_OFFSET / device.block_size() as u64, &mut sb_buf)
            .map_err(|_| VfsError::IoError)?;

        let superblock: Ext2Superblock = unsafe {
            core::ptr::read(sb_buf.as_ptr() as *const Ext2Superblock)
        };

        // マジックナンバーをチェック
        if superblock.s_magic != EXT2_MAGIC {
            return Err(VfsError::InvalidArgument);
        }

        let block_size = 1024 << superblock.s_log_block_size;
        
        // inodeサイズを取得（デフォルトは128バイト）
        let inode_size = if superblock.s_rev_level >= 1 {
            superblock.s_inode_size as usize
        } else {
            128
        };

        // ブロックグループディスクリプタテーブルを読み取る
        let num_groups = ((superblock.s_blocks_count + superblock.s_blocks_per_group - 1) 
            / superblock.s_blocks_per_group) as usize;
        
        let gdt_block = if block_size == 1024 { 2 } else { 1 };
        let gdt_size = num_groups * core::mem::size_of::<Ext2GroupDesc>();
        let gdt_blocks = (gdt_size + block_size - 1) / block_size;
        
        let mut gdt_buf = vec![0u8; gdt_blocks * block_size];
        for i in 0..gdt_blocks {
            let mut block_buf = vec![0u8; block_size];
            let blocks_per_fs_block = block_size / device.block_size();
            let start_block = (gdt_block + i) as u64 * blocks_per_fs_block as u64;
            
            for j in 0..blocks_per_fs_block {
                let offset = j * device.block_size();
                device.read_block(start_block + j as u64, &mut block_buf[offset..offset + device.block_size()])
                    .map_err(|_| VfsError::IoError)?;
            }
            gdt_buf[i * block_size..(i + 1) * block_size].copy_from_slice(&block_buf);
        }

        let mut group_desc_table = Vec::new();
        for i in 0..num_groups {
            let offset = i * core::mem::size_of::<Ext2GroupDesc>();
            let desc: Ext2GroupDesc = unsafe {
                core::ptr::read((gdt_buf.as_ptr() as usize + offset) as *const Ext2GroupDesc)
            };
            group_desc_table.push(desc);
        }

        Ok(Self {
            device,
            superblock,
            block_size,
            inodes_per_group: superblock.s_inodes_per_group,
            blocks_per_group: superblock.s_blocks_per_group,
            inode_size,
            group_desc_table,
        })
    }

    /// ブロックを読み取る
    fn read_fs_block(&self, block_num: u32, buf: &mut [u8]) -> VfsResult<()> {
        if buf.len() < self.block_size {
            return Err(VfsError::InvalidArgument);
        }

        // ファイルシステムブロックをデバイスブロックに変換
        let blocks_per_fs_block = self.block_size / self.device.block_size();
        let start_block = block_num as u64 * blocks_per_fs_block as u64;

        for i in 0..blocks_per_fs_block {
            let offset = i * self.device.block_size();
            self.device
                .read_block(start_block + i as u64, &mut buf[offset..offset + self.device.block_size()])
                .map_err(|_| VfsError::IoError)?;
        }

        Ok(())
    }

    /// inodeを読み取る
    fn read_inode(&self, inode_num: u64) -> VfsResult<Ext2Inode> {
        if inode_num == 0 {
            return Err(VfsError::NotFound);
        }

        // inodeが所属するブロックグループを計算
        let inode_idx = inode_num - 1;
        let group = (inode_idx / self.inodes_per_group as u64) as usize;
        let local_idx = inode_idx % self.inodes_per_group as u64;

        if group >= self.group_desc_table.len() {
            return Err(VfsError::NotFound);
        }

        // ブロックグループディスクリプタからinodeテーブルの開始ブロックを取得
        let gd = &self.group_desc_table[group];
        let inode_table_block = gd.bg_inode_table;

        // inode テーブル内のオフセットを計算
        let inode_offset = local_idx as usize * self.inode_size;
        let block_offset = inode_offset / self.block_size;
        let byte_offset = inode_offset % self.block_size;

        // inodeを含むブロックを読み取る
        let mut block_buf = vec![0u8; self.block_size];
        self.read_fs_block(inode_table_block + block_offset as u32, &mut block_buf)?;

        // inodeを抽出
        let inode: Ext2Inode = unsafe {
            core::ptr::read((block_buf.as_ptr() as usize + byte_offset) as *const Ext2Inode)
        };

        Ok(inode)
    }

    /// ブロックポインタからブロック番号を取得
    fn get_block_num(&self, inode: &Ext2Inode, block_idx: u32) -> VfsResult<u32> {
        // 直接ブロックポインタ（0-11）
        if block_idx < 12 {
            return Ok(inode.i_block[block_idx as usize]);
        }

        let ptrs_per_block = (self.block_size / 4) as u32;

        // 間接ブロックポインタ（12）
        if block_idx < 12 + ptrs_per_block {
            let indirect_block = inode.i_block[12];
            if indirect_block == 0 {
                return Ok(0);
            }

            let mut block_buf = vec![0u8; self.block_size];
            self.read_fs_block(indirect_block, &mut block_buf)?;

            let offset = ((block_idx - 12) * 4) as usize;
            let block_num = u32::from_le_bytes([
                block_buf[offset],
                block_buf[offset + 1],
                block_buf[offset + 2],
                block_buf[offset + 3],
            ]);
            return Ok(block_num);
        }

        // 二重間接ブロックポインタ（13）
        if block_idx < 12 + ptrs_per_block + ptrs_per_block * ptrs_per_block {
            let double_indirect = inode.i_block[13];
            if double_indirect == 0 {
                return Ok(0);
            }

            let idx = block_idx - 12 - ptrs_per_block;
            let indirect_idx = idx / ptrs_per_block;
            let block_offset = idx % ptrs_per_block;

            // 最初の間接ブロックを読み取る
            let mut block_buf = vec![0u8; self.block_size];
            self.read_fs_block(double_indirect, &mut block_buf)?;

            let offset = (indirect_idx * 4) as usize;
            let indirect_block = u32::from_le_bytes([
                block_buf[offset],
                block_buf[offset + 1],
                block_buf[offset + 2],
                block_buf[offset + 3],
            ]);

            if indirect_block == 0 {
                return Ok(0);
            }

            // 二番目の間接ブロックを読み取る
            self.read_fs_block(indirect_block, &mut block_buf)?;

            let offset = (block_offset * 4) as usize;
            let block_num = u32::from_le_bytes([
                block_buf[offset],
                block_buf[offset + 1],
                block_buf[offset + 2],
                block_buf[offset + 3],
            ]);
            return Ok(block_num);
        }

        // 三重間接ブロックは未サポート
        Err(VfsError::NotSupported)
    }
}

impl FileSystem for Ext2Fs {
    fn name(&self) -> &str {
        "ext2"
    }

    fn root_inode(&self) -> u64 {
        2 // ext2のルートinodeは常に2
    }

    fn stat(&self, inode: u64) -> VfsResult<FileAttr> {
        let ext2_inode = self.read_inode(inode)?;
        
        let file_type = match ext2_inode.i_mode & 0xF000 {
            EXT2_S_IFREG => FileType::RegularFile,
            EXT2_S_IFDIR => FileType::Directory,
            EXT2_S_IFLNK => FileType::SymbolicLink,
            _ => FileType::RegularFile,
        };

        Ok(FileAttr {
            file_type,
            size: ext2_inode.i_size as u64,
            blocks: ext2_inode.i_blocks as u64,
            atime: ext2_inode.i_atime as u64,
            mtime: ext2_inode.i_mtime as u64,
            ctime: ext2_inode.i_ctime as u64,
            mode: ext2_inode.i_mode,
            uid: ext2_inode.i_uid as u32,
            gid: ext2_inode.i_gid as u32,
            nlink: ext2_inode.i_links_count as u32,
        })
    }

    fn lookup(&self, parent_inode: u64, name: &str) -> VfsResult<u64> {
        let parent = self.read_inode(parent_inode)?;
        
        // ディレクトリかチェック
        if parent.i_mode & 0xF000 != EXT2_S_IFDIR {
            return Err(VfsError::NotDirectory);
        }

        // ディレクトリの内容を読み取る
        let size = parent.i_size as usize;
        let mut data = vec![0u8; size];
        
        let mut read_offset = 0;
        let mut block_idx = 0;
        
        while read_offset < size {
            let block_num = self.get_block_num(&parent, block_idx)?;
            if block_num == 0 {
                break;
            }
            
            let mut block_buf = vec![0u8; self.block_size];
            self.read_fs_block(block_num, &mut block_buf)?;
            
            let to_copy = core::cmp::min(self.block_size, size - read_offset);
            data[read_offset..read_offset + to_copy].copy_from_slice(&block_buf[..to_copy]);
            
            read_offset += to_copy;
            block_idx += 1;
        }

        // ディレクトリエントリを走査
        let mut offset = 0;
        while offset < size {
            if offset + core::mem::size_of::<Ext2DirEntry>() > size {
                break;
            }

            let entry: Ext2DirEntry = unsafe {
                core::ptr::read((data.as_ptr() as usize + offset) as *const Ext2DirEntry)
            };

            if entry.rec_len == 0 {
                break;
            }

            if entry.inode != 0 && entry.name_len > 0 {
                let name_offset = offset + core::mem::size_of::<Ext2DirEntry>();
                if name_offset + entry.name_len as usize <= size {
                    let entry_name = &data[name_offset..name_offset + entry.name_len as usize];
                    
                    if let Ok(entry_name_str) = core::str::from_utf8(entry_name) {
                        if entry_name_str == name {
                            return Ok(entry.inode as u64);
                        }
                    }
                }
            }

            offset += entry.rec_len as usize;
        }

        Err(VfsError::NotFound)
    }

    fn read(&self, inode: u64, offset: u64, buf: &mut [u8]) -> VfsResult<usize> {
        let ext2_inode = self.read_inode(inode)?;
        
        // 通常ファイルかチェック
        if ext2_inode.i_mode & 0xF000 != EXT2_S_IFREG {
            return Err(VfsError::IsDirectory);
        }

        let file_size = ext2_inode.i_size as u64;
        
        if offset >= file_size {
            return Ok(0); // EOF
        }

        let to_read = core::cmp::min(buf.len(), (file_size - offset) as usize);
        
        let start_block = (offset / self.block_size as u64) as u32;
        let block_offset = (offset % self.block_size as u64) as usize;
        
        let mut bytes_read = 0;
        let mut current_block = start_block;
        
        while bytes_read < to_read {
            let block_num = self.get_block_num(&ext2_inode, current_block)?;
            if block_num == 0 {
                // スパースファイル - ゼロで埋める
                let remaining = to_read - bytes_read;
                let to_zero = core::cmp::min(remaining, self.block_size - block_offset);
                buf[bytes_read..bytes_read + to_zero].fill(0);
                bytes_read += to_zero;
            } else {
                let mut block_buf = vec![0u8; self.block_size];
                self.read_fs_block(block_num, &mut block_buf)?;
                
                let start = if current_block == start_block { block_offset } else { 0 };
                let remaining = to_read - bytes_read;
                let to_copy = core::cmp::min(remaining, self.block_size - start);
                
                buf[bytes_read..bytes_read + to_copy].copy_from_slice(&block_buf[start..start + to_copy]);
                bytes_read += to_copy;
            }
            
            current_block += 1;
        }

        Ok(bytes_read)
    }

    fn write(&mut self, _inode: u64, _offset: u64, _buf: &[u8]) -> VfsResult<usize> {
        // TODO: ファイル書き込みを実装（読み取り専用の場合はエラー）
        Err(VfsError::ReadOnlyFs)
    }

    fn readdir(&self, inode: u64) -> VfsResult<Vec<DirEntry>> {
        let ext2_inode = self.read_inode(inode)?;
        
        // ディレクトリかチェック
        if ext2_inode.i_mode & 0xF000 != EXT2_S_IFDIR {
            return Err(VfsError::NotDirectory);
        }

        let size = ext2_inode.i_size as usize;
        let mut data = vec![0u8; size];
        
        // ディレクトリの内容を読み取る
        let mut read_offset = 0;
        let mut block_idx = 0;
        
        while read_offset < size {
            let block_num = self.get_block_num(&ext2_inode, block_idx)?;
            if block_num == 0 {
                break;
            }
            
            let mut block_buf = vec![0u8; self.block_size];
            self.read_fs_block(block_num, &mut block_buf)?;
            
            let to_copy = core::cmp::min(self.block_size, size - read_offset);
            data[read_offset..read_offset + to_copy].copy_from_slice(&block_buf[..to_copy]);
            
            read_offset += to_copy;
            block_idx += 1;
        }

        // ディレクトリエントリを解析
        let mut entries = Vec::new();
        let mut offset = 0;
        
        while offset < size {
            if offset + core::mem::size_of::<Ext2DirEntry>() > size {
                break;
            }

            let entry: Ext2DirEntry = unsafe {
                core::ptr::read((data.as_ptr() as usize + offset) as *const Ext2DirEntry)
            };

            if entry.rec_len == 0 {
                break;
            }

            if entry.inode != 0 && entry.name_len > 0 {
                let name_offset = offset + core::mem::size_of::<Ext2DirEntry>();
                if name_offset + entry.name_len as usize <= size {
                    let entry_name = &data[name_offset..name_offset + entry.name_len as usize];
                    
                    if let Ok(name_str) = core::str::from_utf8(entry_name) {
                        let file_type = match entry.file_type {
                            1 => FileType::RegularFile,
                            2 => FileType::Directory,
                            7 => FileType::SymbolicLink,
                            _ => FileType::RegularFile,
                        };
                        
                        entries.push(DirEntry {
                            name: String::from(name_str),
                            inode: entry.inode as u64,
                            file_type,
                        });
                    }
                }
            }

            offset += entry.rec_len as usize;
        }

        Ok(entries)
    }

    fn create(&mut self, _parent_inode: u64, _name: &str, _mode: u16) -> VfsResult<u64> {
        Err(VfsError::ReadOnlyFs)
    }

    fn mkdir(&mut self, _parent_inode: u64, _name: &str, _mode: u16) -> VfsResult<u64> {
        Err(VfsError::ReadOnlyFs)
    }

    fn unlink(&mut self, _parent_inode: u64, _name: &str) -> VfsResult<()> {
        Err(VfsError::ReadOnlyFs)
    }

    fn rmdir(&mut self, _parent_inode: u64, _name: &str) -> VfsResult<()> {
        Err(VfsError::ReadOnlyFs)
    }

    fn truncate(&mut self, _inode: u64, _size: u64) -> VfsResult<()> {
        Err(VfsError::ReadOnlyFs)
    }

    fn sync(&mut self) -> VfsResult<()> {
        Ok(())
    }
}
