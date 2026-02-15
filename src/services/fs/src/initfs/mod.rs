//! InitFS - メモリ内ファイルシステム
//!
//! システム起動時に使用される簡易的なRAMベースファイルシステム
//! 猫も杓子もInitFS！（？？？

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::common::vfs::{
    DirEntry, FileAttr, FileSystem, FileType, VfsError, VfsResult,
};

const MAX_FILES: usize = 64;
const FILE_SIZE: usize = 4096;

/// InitFSのinode
#[derive(Clone)]
struct InitFsInode {
    used: bool,
    inode_num: u64,
    file_type: FileType,
    name: String,
    data: Vec<u8>,
    size: u64,
    parent: u64,
}

impl InitFsInode {
    fn new_empty() -> Self {
        Self {
            used: false,
            inode_num: 0,
            file_type: FileType::RegularFile,
            name: String::new(),
            data: Vec::new(),
            size: 0,
            parent: 0,
        }
    }

    fn new_file(inode_num: u64, name: String, parent: u64) -> Self {
        Self {
            used: true,
            inode_num,
            file_type: FileType::RegularFile,
            name,
            data: Vec::new(),
            size: 0,
            parent,
        }
    }

    fn new_dir(inode_num: u64, name: String, parent: u64) -> Self {
        Self {
            used: true,
            inode_num,
            file_type: FileType::Directory,
            name,
            data: Vec::new(),
            size: 0,
            parent,
        }
    }
}

/// InitFS実装
pub struct InitFs {
    inodes: Vec<InitFsInode>,
    next_inode: AtomicU64,
}

impl InitFs {
    const ROOT_INODE: u64 = 1;

    pub fn new() -> Self {
        let mut inodes = Vec::new();
        
        // inode 0は未使用
        inodes.push(InitFsInode::new_empty());
        
        // inode 1はルートディレクトリ
        inodes.push(InitFsInode::new_dir(
            Self::ROOT_INODE,
            String::from("/"),
            Self::ROOT_INODE, // ルートの親は自分自身
        ));
        
        // 残りを初期化
        for _ in 2..MAX_FILES {
            inodes.push(InitFsInode::new_empty());
        }

        Self {
            inodes,
            next_inode: AtomicU64::new(2),
        }
    }

    /// サンプルファイルを作成
    pub fn create_sample_files(&mut self) -> VfsResult<()> {
        // readme.txtを作成
        let readme_inode = self.create(Self::ROOT_INODE, "readme.txt", 0o644)?;
        let content = b"Welcome to SwiftCore OS!\nThis file is served by InitFS (VFS version).\n";
        self.write(readme_inode, 0, content)?;

        // hello.txtも作成
        let hello_inode = self.create(Self::ROOT_INODE, "hello.txt", 0o644)?;
        let hello_content = b"Hello from InitFS!\n";
        self.write(hello_inode, 0, hello_content)?;

        Ok(())
    }

    fn allocate_inode(&self) -> VfsResult<u64> {
        let inode_num = self.next_inode.fetch_add(1, Ordering::SeqCst);
        if (inode_num as usize) >= MAX_FILES {
            return Err(VfsError::OutOfSpace);
        }
        Ok(inode_num)
    }

    fn get_inode(&self, inode: u64) -> VfsResult<&InitFsInode> {
        if (inode as usize) < self.inodes.len() && self.inodes[inode as usize].used {
            Ok(&self.inodes[inode as usize])
        } else {
            Err(VfsError::NotFound)
        }
    }

    fn get_inode_mut(&mut self, inode: u64) -> VfsResult<&mut InitFsInode> {
        if (inode as usize) < self.inodes.len() && self.inodes[inode as usize].used {
            Ok(&mut self.inodes[inode as usize])
        } else {
            Err(VfsError::NotFound)
        }
    }
}

impl FileSystem for InitFs {
    fn name(&self) -> &str {
        "initfs"
    }

    fn root_inode(&self) -> u64 {
        Self::ROOT_INODE
    }

    fn stat(&self, inode: u64) -> VfsResult<FileAttr> {
        let node = self.get_inode(inode)?;
        
        Ok(FileAttr {
            file_type: node.file_type,
            size: node.size,
            blocks: (node.size + 511) / 512,
            atime: 0,
            mtime: 0,
            ctime: 0,
            mode: match node.file_type {
                FileType::Directory => 0o755,
                _ => 0o644,
            },
            uid: 0,
            gid: 0,
            nlink: 1,
        })
    }

    fn lookup(&self, parent_inode: u64, name: &str) -> VfsResult<u64> {
        // 親がディレクトリか確認
        let parent = self.get_inode(parent_inode)?;
        if parent.file_type != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }

        // すべてのinodeを検索
        for node in &self.inodes {
            if node.used && node.parent == parent_inode && node.name == name {
                return Ok(node.inode_num);
            }
        }

        Err(VfsError::NotFound)
    }

    fn read(&self, inode: u64, offset: u64, buf: &mut [u8]) -> VfsResult<usize> {
        let node = self.get_inode(inode)?;
        
        if node.file_type != FileType::RegularFile {
            return Err(VfsError::IsDirectory);
        }

        let offset = offset as usize;
        if offset >= node.data.len() {
            return Ok(0); // EOF
        }

        let available = node.data.len() - offset;
        let to_read = core::cmp::min(buf.len(), available);
        
        buf[..to_read].copy_from_slice(&node.data[offset..offset + to_read]);
        
        Ok(to_read)
    }

    fn write(&mut self, inode: u64, offset: u64, buf: &[u8]) -> VfsResult<usize> {
        let node = self.get_inode_mut(inode)?;
        
        if node.file_type != FileType::RegularFile {
            return Err(VfsError::IsDirectory);
        }

        let offset = offset as usize;
        let end_offset = offset + buf.len();

        // 必要に応じてバッファを拡張
        if end_offset > node.data.len() {
            if end_offset > FILE_SIZE {
                return Err(VfsError::FileTooBig);
            }
            node.data.resize(end_offset, 0);
        }

        node.data[offset..end_offset].copy_from_slice(buf);
        node.size = node.data.len() as u64;

        Ok(buf.len())
    }

    fn readdir(&self, inode: u64) -> VfsResult<Vec<DirEntry>> {
        let node = self.get_inode(inode)?;
        
        if node.file_type != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }

        let mut entries = Vec::new();
        
        // . と .. を追加
        entries.push(DirEntry {
            name: String::from("."),
            inode,
            file_type: FileType::Directory,
        });
        
        entries.push(DirEntry {
            name: String::from(".."),
            inode: node.parent,
            file_type: FileType::Directory,
        });

        // 子エントリを検索
        for child in &self.inodes {
            if child.used && child.parent == inode {
                entries.push(DirEntry {
                    name: child.name.clone(),
                    inode: child.inode_num,
                    file_type: child.file_type,
                });
            }
        }

        Ok(entries)
    }

    fn create(&mut self, parent_inode: u64, name: &str, _mode: u16) -> VfsResult<u64> {
        // 親がディレクトリか確認
        let _parent = self.get_inode(parent_inode)?;
        
        // 既存ファイルをチェック
        if self.lookup(parent_inode, name).is_ok() {
            return Err(VfsError::AlreadyExists);
        }

        // 新しいinodeを割り当て
        let inode_num = self.allocate_inode()?;
        
        let mut new_file = InitFsInode::new_file(
            inode_num,
            String::from(name),
            parent_inode,
        );
        new_file.used = true;
        
        self.inodes[inode_num as usize] = new_file;
        
        Ok(inode_num)
    }

    fn mkdir(&mut self, parent_inode: u64, name: &str, _mode: u16) -> VfsResult<u64> {
        // 親がディレクトリか確認
        let _parent = self.get_inode(parent_inode)?;
        
        // 既存ディレクトリをチェック
        if self.lookup(parent_inode, name).is_ok() {
            return Err(VfsError::AlreadyExists);
        }

        // 新しいinodeを割り当て
        let inode_num = self.allocate_inode()?;
        
        let mut new_dir = InitFsInode::new_dir(
            inode_num,
            String::from(name),
            parent_inode,
        );
        new_dir.used = true;
        
        self.inodes[inode_num as usize] = new_dir;
        
        Ok(inode_num)
    }

    fn unlink(&mut self, parent_inode: u64, name: &str) -> VfsResult<()> {
        let inode = self.lookup(parent_inode, name)?;
        let node = self.get_inode(inode)?;
        
        if node.file_type == FileType::Directory {
            return Err(VfsError::IsDirectory);
        }

        self.inodes[inode as usize].used = false;
        Ok(())
    }

    fn rmdir(&mut self, parent_inode: u64, name: &str) -> VfsResult<()> {
        let inode = self.lookup(parent_inode, name)?;
        let node = self.get_inode(inode)?;
        
        if node.file_type != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }

        // ディレクトリが空か確認
        let entries = self.readdir(inode)?;
        if entries.len() > 2 {  // . と .. 以外がある
            return Err(VfsError::NotSupported); // ENOTEMPTY的なエラー
        }

        self.inodes[inode as usize].used = false;
        Ok(())
    }

    fn truncate(&mut self, inode: u64, size: u64) -> VfsResult<()> {
        let node = self.get_inode_mut(inode)?;
        
        if node.file_type != FileType::RegularFile {
            return Err(VfsError::IsDirectory);
        }

        node.data.resize(size as usize, 0);
        node.size = size;
        
        Ok(())
    }

    fn sync(&mut self) -> VfsResult<()> {
        // メモリ上なので何もしない。だって再起動したら全部どっか行ってるし書き込みできないから
        Ok(())
    }
}
