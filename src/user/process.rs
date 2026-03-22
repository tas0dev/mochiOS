//! プロセス管理関連のシステムコール

use super::sys::{syscall2, SyscallNumber};

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
    
    let result = syscall2(
        SyscallNumber::Exec as u64,
        path_buf.as_ptr() as u64,
        0,
    );

    if (result as i64) < 0 {
        Err(())
    } else {
        Ok(result)
    }
}

/// 引数付きで実行可能ファイルを起動する
/// args: 各引数のスライス（argv[0] = path は自動で設定）
pub fn exec_with_args(path: &str, args: &[&str]) -> Result<u64, ()> {
    let mut path_buf = [0u8; 256];
    let path_bytes = path.as_bytes();
    if path_bytes.len() >= 255 {
        return Err(());
    }
    path_buf[..path_bytes.len()].copy_from_slice(path_bytes);
    path_buf[path_bytes.len()] = 0;

    // ヌル区切り引数文字列: "arg1\0arg2\0\0"
    let mut args_buf = [0u8; 512];
    let mut pos = 0usize;
    for arg in args {
        let b = arg.as_bytes();
        if pos + b.len() + 2 > args_buf.len() {
            break;
        }
        args_buf[pos..pos + b.len()].copy_from_slice(b);
        pos += b.len();
        args_buf[pos] = 0; // null terminate arg
        pos += 1;
    }
    args_buf[pos] = 0; // double-null = end of args

    let result = syscall2(
        SyscallNumber::Exec as u64,
        path_buf.as_ptr() as u64,
        if args.is_empty() { 0 } else { args_buf.as_ptr() as u64 },
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

/// メモリ上の ELF データと実行パス名から新プロセスを起動する
pub fn exec_from_buffer_named(path: &str, elf_data: &[u8]) -> Result<u64, i64> {
    use super::sys::syscall3;

    let mut path_buf = [0u8; 256];
    let path_bytes = path.as_bytes();
    if path_bytes.is_empty() || path_bytes.len() >= 255 {
        return Err(-22);
    }
    path_buf[..path_bytes.len()].copy_from_slice(path_bytes);
    path_buf[path_bytes.len()] = 0;

    let result = syscall3(
        SyscallNumber::ExecFromBufferNamed as u64,
        elf_data.as_ptr() as u64,
        elf_data.len() as u64,
        path_buf.as_ptr() as u64,
    );
    if (result as i64) < 0 {
        Err(result as i64)
    } else {
        Ok(result)
    }
}

/// メモリ上の ELF データと実行パス名・引数から新プロセスを起動する
pub fn exec_from_buffer_named_with_args(
    path: &str,
    elf_data: &[u8],
    args: &[&str],
) -> Result<u64, i64> {
    use super::sys::syscall4;

    let mut path_buf = [0u8; 256];
    let path_bytes = path.as_bytes();
    if path_bytes.is_empty() || path_bytes.len() >= 255 {
        return Err(-22);
    }
    path_buf[..path_bytes.len()].copy_from_slice(path_bytes);
    path_buf[path_bytes.len()] = 0;

    // ヌル区切り引数文字列: "arg1\0arg2\0\0"
    let mut args_buf = [0u8; 512];
    let mut pos = 0usize;
    for arg in args {
        let b = arg.as_bytes();
        if pos + b.len() + 2 > args_buf.len() {
            return Err(-22);
        }
        args_buf[pos..pos + b.len()].copy_from_slice(b);
        pos += b.len();
        args_buf[pos] = 0;
        pos += 1;
    }
    args_buf[pos] = 0;
    let args_ptr = if args.is_empty() { 0 } else { args_buf.as_ptr() as u64 };

    let result = syscall4(
        SyscallNumber::ExecFromBufferNamedArgs as u64,
        elf_data.as_ptr() as u64,
        elf_data.len() as u64,
        path_buf.as_ptr() as u64,
        args_ptr,
    );
    if (result as i64) < 0 {
        Err(result as i64)
    } else {
        Ok(result)
    }
}
