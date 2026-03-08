//! プロセス管理関連のシステムコール

use super::sys::{syscall1, SyscallNumber};

/// 実行可能ファイルを起動する
/// パスから新しいプロセスを起動し、そのPIDを返す
pub fn exec(path: &str) -> Result<u64, ()> {
    // null終端文字列を作成
    let mut path_buf = [0u8; 256];
    let path_bytes = path.as_bytes();
    if path_bytes.len() >= 255 {
        return Err(());
    }
    path_buf[..path_bytes.len()].copy_from_slice(path_bytes);
    path_buf[path_bytes.len()] = 0;
    
    let result = syscall1(
        SyscallNumber::Exec as u64,
        path_buf.as_ptr() as u64,
    );

    if (result as i64) < 0 {
        Err(())
    } else {
        Ok(result)
    }
}

/// メモリ上の ELF データから新プロセスを起動する
pub fn exec_from_buffer(elf_data: &[u8]) -> Result<u64, ()> {
    use super::sys::syscall2;
    let result = syscall2(
        SyscallNumber::ExecFromBuffer as u64,
        elf_data.as_ptr() as u64,
        elf_data.len() as u64,
    );
    if (result as i64) < 0 {
        Err(())
    } else {
        Ok(result)
    }
}
