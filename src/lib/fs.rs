use crate::alloc_crate::string::String;
use crate::ipc;
use crate::process;
use crate::thread;
use core::mem::size_of;

/// FSのリクエスト/レスポンス構造体
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FsRequest {
    /// 操作コード
    op: u64,
    /// 引数1
    arg1: u64,
    /// 引数2
    arg2: u64,
    /// パスまたはデータ
    path: [u8; 128],
}

impl FsRequest {
    const OP_OPEN: u64 = 1;
    const OP_READ: u64 = 2;
    const OP_WRITE: u64 = 3;
    const OP_CLOSE: u64 = 4;
}

/// FSのレスポンス構造体
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FsResponse {
    /// ステータスコード
    status: i64,
    /// データ長
    len: u64,
    /// データ
    data: [u8; 128],
}

static mut FS_PID: u64 = 0;

fn get_fs_pid() -> Option<u64> {
    unsafe {
        if FS_PID != 0 {
            return Some(FS_PID);
        }
        // Try to find fs.service
        for _ in 0..10 {
            if let Some(pid) = process::find_by_name("core.service.fs") {
                FS_PID = pid;
                return Some(pid);
            }
            thread::sleep(50);
        }
        None
    }
}

// --- Public API ---

#[derive(Debug)]
pub enum FsError {
    ServiceNotFound,
    FileNotFound,
    PermissionDenied,
    IoError,
    Other(i64),
}

pub type Result<T> = core::result::Result<T, FsError>;

pub struct File {
    fd: u64,
}

impl File {
    pub fn open(path: &str) -> Result<Self> {
        let fs_pid = get_fs_pid().ok_or(FsError::ServiceNotFound)?;

        let mut req = FsRequest {
            op: FsRequest::OP_OPEN,
            arg1: 0,
            arg2: 0,
            path: [0; 128],
        };

        for (i, b) in path.bytes().enumerate() {
            if i < 128 { req.path[i] = b; }
        }

        let req_slice = unsafe {
            core::slice::from_raw_parts(&req as *const _ as *const u8, size_of::<FsRequest>())
        };

        if ipc::send(fs_pid, req_slice).is_err() {
            return Err(FsError::IoError);
        }

        let mut resp_buf = [0u8; 256];
        loop {
            let (sender, len) = ipc::recv(&mut resp_buf);
            if sender == fs_pid && len >= size_of::<FsResponse>() {
                let resp: FsResponse = unsafe { core::ptr::read(resp_buf.as_ptr() as *const _) };
                if resp.status >= 0 {
                    return Ok(File { fd: resp.status as u64 });
                } else {
                    return match resp.status {
                        // -2 is ENOENT in RamFS?
                        _ => Err(FsError::FileNotFound), // Simply map all errors to FileNotFound for now
                    };
                }
            }
            thread::yield_now();
        }
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let fs_pid = get_fs_pid().ok_or(FsError::ServiceNotFound)?;

        let read_len = if buf.len() > 128 { 128 } else { buf.len() };

        let req = FsRequest {
            op: FsRequest::OP_READ,
            arg1: self.fd,
            arg2: read_len as u64,
            path: [0; 128],
        };

        let req_slice = unsafe {
            core::slice::from_raw_parts(&req as *const _ as *const u8, size_of::<FsRequest>())
        };

        if ipc::send(fs_pid, req_slice).is_err() {
            return Err(FsError::IoError);
        }

        let mut resp_buf = [0u8; 256];
        loop {
             let (sender, len) = ipc::recv(&mut resp_buf);
             if sender == fs_pid && len >= size_of::<FsResponse>() {
                 let resp: FsResponse = unsafe { core::ptr::read(resp_buf.as_ptr() as *const _) };
                 if resp.status >= 0 {
                     let data_len = resp.len as usize;
                     for i in 0..data_len {
                         if i < buf.len() {
                             buf[i] = resp.data[i];
                         }
                     }
                     return Ok(data_len);
                 } else {
                     return Err(FsError::IoError);
                 }
             }
             thread::yield_now();
         }
    }

    pub fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        let mut temp_buf = [0u8; 128];
        let mut total_read = 0;

        loop {
            let n = self.read(&mut temp_buf)?;
            if n == 0 {
                break;
            }
            if let Ok(s) = core::str::from_utf8(&temp_buf[..n]) {
                buf.push_str(s);
                total_read += n;
            } else {
                return Err(FsError::IoError);
            }
            if n < 128 {
                break;
            }
        }
        Ok(total_read)
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize> {
         let fs_pid = get_fs_pid().ok_or(FsError::ServiceNotFound)?;

         let write_len = if buf.len() > 128 { 128 } else { buf.len() };

         let mut req = FsRequest {
             op: FsRequest::OP_WRITE,
             arg1: self.fd,
             arg2: write_len as u64,
             path: [0; 128],
         };

         for i in 0..write_len {
             req.path[i] = buf[i];
         }

         let req_slice = unsafe {
             core::slice::from_raw_parts(&req as *const _ as *const u8, size_of::<FsRequest>())
         };

         if ipc::send(fs_pid, req_slice).is_err() {
             return Err(FsError::IoError);
         }

         let mut resp_buf = [0u8; 256];
         loop {
             let (sender, len) = ipc::recv(&mut resp_buf);
             if sender == fs_pid && len >= size_of::<FsResponse>() {
                 let resp: FsResponse = unsafe { core::ptr::read(resp_buf.as_ptr() as *const _) };
                 if resp.status >= 0 {
                     return Ok(resp.len as usize);
                 } else {
                     return Err(FsError::IoError);
                 }
             }
             thread::yield_now();
         }
    }
}

impl Drop for File {
    fn drop(&mut self) {
        if let Some(fs_pid) = get_fs_pid() {
             let req = FsRequest {
                op: FsRequest::OP_CLOSE,
                arg1: self.fd,
                arg2: 0,
                path: [0; 128],
            };
            let req_slice = unsafe {
                core::slice::from_raw_parts(&req as *const _ as *const u8, size_of::<FsRequest>())
            };
            let _ = ipc::send(fs_pid, req_slice);
        }
    }
}
