pub mod vfs;

pub use vfs::{
    resolve_path, FileHandle, FileSystem,
    VfsError,
};
