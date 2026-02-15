//! Virtual File System (VFS) 抽象化レイヤー
//!
//! 複数のファイルシステム実装を統一的に扱うための共通インターフェース

use alloc::string::String;
use alloc::vec::Vec;

/// ファイルタイプ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    /// 通常のファイル
    RegularFile,
    /// ディレクトリ
    Directory,
    /// シンボリックリンク
    SymbolicLink,
    /// ブロックデバイス
    BlockDevice,
    /// キャラクターデバイス
    CharDevice,
    /// FIFO（名前付きパイプ）
    Fifo,
    /// ソケット
    Socket,
}

/// ファイル属性
#[derive(Debug, Clone)]
pub struct FileAttr {
    /// ファイルタイプ
    pub file_type: FileType,
    /// ファイルサイズ
    pub size: u64,
    /// ブロック数
    pub blocks: u64,
    /// アクセスタイム（最終アクセス日時）
    pub atime: u64,
    /// モディファイタイム（最終更新日時）
    pub mtime: u64,
    /// クリエイトタイム（作成日時）
    pub ctime: u64,
    /// パーミッション
    pub mode: u16,
    /// ユーザーID
    pub uid: u32,
    /// グループID
    pub gid: u32,
    /// ハードリンク数
    pub nlink: u32,
}

/// VFSエラー
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    NotFound,           // ファイルが存在しない
    PermissionDenied,   // 権限がない
    AlreadyExists,      // すでに存在する
    IsDirectory,        // ディレクトリである
    NotDirectory,       // ディレクトリでない
    InvalidArgument,    // 引数が不正
    IoError,            // I/Oエラー
    OutOfSpace,         // 容量不足
    ReadOnlyFs,         // 読み取り専用
    TooManyOpenFiles,   // オープンファイル数上限
    FileTooBig,         // ファイルが大きすぎる
    NotSupported,       // 未対応の操作
}

pub type VfsResult<T> = Result<T, VfsError>;

/// ディレクトリエントリ
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// エントリ名
    pub name: String,
    /// 対応するinode番号
    pub inode: u64,
    /// ファイルタイプ
    pub file_type: FileType,
}

/// ファイルシステムトレイト
///
/// 各ファイルシステム実装が実装すべきインターフェース
pub trait FileSystem: Send + Sync {
    /// ファイルシステム名を取得
    fn name(&self) -> &str;

    /// ルートinode番号を取得
    fn root_inode(&self) -> u64;

    /// inodeからファイル属性を取得
    fn stat(&self, inode: u64) -> VfsResult<FileAttr>;

    /// パスからinodeを検索
    fn lookup(&self, parent_inode: u64, name: &str) -> VfsResult<u64>;

    /// ファイルを読み取る
    fn read(&self, inode: u64, offset: u64, buf: &mut [u8]) -> VfsResult<usize>;

    /// ファイルに書き込む
    fn write(&mut self, inode: u64, offset: u64, buf: &[u8]) -> VfsResult<usize>;

    /// ディレクトリの内容を読む
    fn readdir(&self, inode: u64) -> VfsResult<Vec<DirEntry>>;

    /// ファイルを作成
    fn create(&mut self, parent_inode: u64, name: &str, mode: u16) -> VfsResult<u64>;

    /// ディレクトリを作成
    fn mkdir(&mut self, parent_inode: u64, name: &str, mode: u16) -> VfsResult<u64>;

    /// ファイル/ディレクトリを削除
    fn unlink(&mut self, parent_inode: u64, name: &str) -> VfsResult<()>;

    /// ディレクトリを削除
    fn rmdir(&mut self, parent_inode: u64, name: &str) -> VfsResult<()>;

    /// ファイルサイズを変更
    fn truncate(&mut self, inode: u64, size: u64) -> VfsResult<()>;

    /// 同期（変更をディスクに書き込む）
    fn sync(&mut self) -> VfsResult<()>;
}

/// ファイルハンドル
///
/// オープンされたファイルの状態を保持
#[derive(Debug, Clone, Copy)]
pub struct FileHandle {
    pub inode: u64,
    pub offset: u64,
    pub flags: u32,
}

impl FileHandle {
    pub fn new(inode: u64, flags: u32) -> Self {
        Self {
            inode,
            offset: 0,
            flags,
        }
    }
}

/// パスを構成要素に分割
pub fn split_path(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .collect()
}

/// パスからinodeを解決
///
/// ファイルシステムのルートからパスを辿ってinodeを取得
pub fn resolve_path(fs: &dyn FileSystem, path: &str) -> VfsResult<u64> {
    let components = split_path(path);
    
    if components.is_empty() {
        // ルートディレクトリ
        return Ok(fs.root_inode());
    }

    let mut current_inode = fs.root_inode();
    
    for component in components {
        // 現在のinodeがディレクトリか確認
        let attr = fs.stat(current_inode)?;
        if attr.file_type != FileType::Directory {
            return Err(VfsError::NotDirectory);
        }
        
        // 次の要素を検索
        current_inode = fs.lookup(current_inode, component)?;
    }
    
    Ok(current_inode)
}
