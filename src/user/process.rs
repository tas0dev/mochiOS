//! プロセス管理関連のシステムコール

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

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

/// メモリ上の ELF データと実行パス名・引数・要求元スレッドIDから新プロセスを起動する
pub fn exec_from_buffer_named_with_args_and_requester(
    path: &str,
    elf_data: &[u8],
    args: &[&str],
    requester_tid: u64,
) -> Result<u64, i64> {
    use super::sys::syscall5;

    let mut path_buf = [0u8; 256];
    let path_bytes = path.as_bytes();
    if path_bytes.is_empty() || path_bytes.len() >= 255 {
        return Err(-22);
    }
    path_buf[..path_bytes.len()].copy_from_slice(path_bytes);
    path_buf[path_bytes.len()] = 0;

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

    let result = syscall5(
        SyscallNumber::ExecFromBufferNamedArgsWithRequester as u64,
        elf_data.as_ptr() as u64,
        elf_data.len() as u64,
        path_buf.as_ptr() as u64,
        args_ptr,
        requester_tid,
    );
    if (result as i64) < 0 {
        Err(result as i64)
    } else {
        Ok(result)
    }
}

/// Request kernel to exec via streamed image path (mapped-write zero-copy preferred)
pub fn exec_via_fs_stream(path: &str, args: &[&str]) -> Result<u64, i64> {
    use super::sys::syscall2;

    let mut path_buf = [0u8; 256];
    let path_bytes = path.as_bytes();
    if path_bytes.is_empty() || path_bytes.len() >= 255 {
        return Err(-22);
    }
    path_buf[..path_bytes.len()].copy_from_slice(path_bytes);
    path_buf[path_bytes.len()] = 0;

    // build nul-separated args buffer
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

    let res = syscall2(
        SyscallNumber::ExecFromFsStream as u64,
        path_buf.as_ptr() as u64,
        args_ptr,
    );
    if (res as i64) < 0 { Err(res as i64) } else { Ok(res) }
}

/// プロセス一覧の1件
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u64,
    pub tid: u64,
    pub state: u64,
    pub name: String,
}

/// 現在のプロセス一覧を取得する
pub fn list_processes() -> Vec<ProcessInfo> {
    const RECORD_SIZE: usize = 88;
    let mut buf = vec![0u8; RECORD_SIZE * 128];
    let written = syscall2(
        SyscallNumber::ListProcesses as u64,
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
    ) as usize;
    let count = core::cmp::min(written, buf.len() / RECORD_SIZE);
    let mut out = Vec::with_capacity(count);

    for i in 0..count {
        let off = i * RECORD_SIZE;
        let mut pid_bytes = [0u8; 8];
        let mut tid_bytes = [0u8; 8];
        let mut state_bytes = [0u8; 8];
        pid_bytes.copy_from_slice(&buf[off..off + 8]);
        tid_bytes.copy_from_slice(&buf[off + 8..off + 16]);
        state_bytes.copy_from_slice(&buf[off + 16..off + 24]);
        let name_slice = &buf[off + 32..off + 96];
        let name_len = name_slice
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(name_slice.len());
        out.push(ProcessInfo {
            pid: u64::from_ne_bytes(pid_bytes),
            tid: u64::from_ne_bytes(tid_bytes),
            state: u64::from_ne_bytes(state_bytes),
            name: String::from_utf8_lossy(&name_slice[..name_len]).into_owned(),
        });
    }

    out
}

/// PID からプロセス名を取得する
pub fn process_name_by_pid(pid: u64) -> Option<String> {
    list_processes()
        .into_iter()
        .find(|proc| proc.pid == pid)
        .map(|proc| proc.name)
}
