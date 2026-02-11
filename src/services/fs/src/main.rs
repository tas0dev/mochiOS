#![no_std]
#![no_main]

#[macro_use]
extern crate std;

use core::panic::PanicInfo;
use core::mem::size_of;
use std::{ipc, thread};

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FsRequest {
    op: u64,
    arg1: u64,
    arg2: u64,
    path: [u8; 128],
}
impl FsRequest {
    const OP_OPEN: u64 = 1;
    const OP_READ: u64 = 2;
    const OP_WRITE: u64 = 3;
    const OP_CLOSE: u64 = 4;
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FsResponse {
    status: i64,
    len: u64,
    data: [u8; 128],
}

const MAX_FILES: usize = 4;
const FILE_SIZE: usize = 512;
const MAX_HANDLES: usize = 16;

#[derive(Clone, Copy)]
struct VirtualFile {
    used: bool,
    name: [u8; 32],
    name_len: usize,
    data: [u8; FILE_SIZE],
    size: usize,
}

impl VirtualFile {
    const fn new() -> Self {
        Self { used: false, name: [0; 32], name_len: 0, data: [0; FILE_SIZE], size: 0 }
    }
}

#[derive(Clone, Copy)]
struct FileHandle {
    used: bool,
    file_idx: usize,
    offset: usize,
}

impl FileHandle {
    const fn new() -> Self {
        Self { used: false, file_idx: 0, offset: 0 }
    }
}

static mut FILES: [VirtualFile; MAX_FILES] = [VirtualFile::new(); MAX_FILES];
static mut HANDLES: [FileHandle; MAX_HANDLES] = [FileHandle::new(); MAX_HANDLES];

#[repr(align(8))]
struct AlignedBuffer([u8; 256]);

/// FS Service Entry Point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    swiftcore_std::heap::init();
    println!("[FS] Service Started with swift_std.");

    // 初期ファイル作成
    unsafe {
        FILES[0].used = true;
        let name = "readme.txt";
        for (i, b) in name.bytes().enumerate() {
            if i < 32 { FILES[0].name[i] = b; }
        }
        FILES[0].name_len = name.len();

        let content = "Welcome to SwiftCore OS!\nThis file is served by fs.service from RamFS.\n";
        for (i, b) in content.bytes().enumerate() {
            if i < FILE_SIZE {
                FILES[0].data[i] = b;
            }
        }
        FILES[0].size = content.len();
    }

    println!("[FS] RamFS Initialized. 'readme.txt' created.");
    println!("[FS] Waiting for requests...");

    let mut recv_buf = AlignedBuffer([0u8; 256]);

    loop {
        let (sender, len) = ipc::recv(&mut recv_buf.0);
        if sender != 0 && len >= size_of::<FsRequest>() {
            let req: FsRequest = unsafe { core::ptr::read(recv_buf.0.as_ptr() as *const _) };
            println!("[FS] REQ op={} from PID={}", req.op, sender);

            let mut resp = FsResponse { status: -1, len: 0, data: [0; 128] };

            match req.op {
                FsRequest::OP_OPEN => {
                    let mut found_file_idx: i64 = -1;
                    unsafe {
                        for i in 0..MAX_FILES {
                            if FILES[i].used {
                                let name_len = FILES[i].name_len;
                                let mut path_len = 0;
                                while path_len < 128 && req.path[path_len] != 0 {
                                    path_len += 1;
                                }

                                if name_len == path_len {
                                    let mut matched = true;
                                    for k in 0..name_len {
                                        if FILES[i].name[k] != req.path[k] {
                                            matched = false;
                                            break;
                                        }
                                    }
                                    if matched {
                                        found_file_idx = i as i64;
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    if found_file_idx != -1 {
                        let mut handle_idx: i64 = -1;
                        unsafe {
                            for i in 0..MAX_HANDLES {
                                if !HANDLES[i].used {
                                    HANDLES[i].used = true;
                                    HANDLES[i].file_idx = found_file_idx as usize;
                                    HANDLES[i].offset = 0;
                                    handle_idx = i as i64;
                                    break;
                                }
                            }
                        }

                        resp.status = handle_idx;
                        if handle_idx == -1 {
                            println!("[FS] ERROR: No free handles");
                        } else {
                            println!("[FS] Success: FD={}", handle_idx);
                        }
                    } else {
                         println!("[FS] ERROR: File not found");
                         resp.status = -2; // ENOENT
                    }
                },
                FsRequest::OP_READ => {
                     let fd = req.arg1 as usize;
                     let read_len = req.arg2 as usize;

                     if fd < MAX_HANDLES && unsafe { HANDLES[fd].used } {
                         let handle = unsafe { &mut HANDLES[fd] };
                         let file_idx = handle.file_idx;

                         if file_idx < MAX_FILES && unsafe { FILES[file_idx].used } {
                            let file_size = unsafe { FILES[file_idx].size };
                            let current_offset = handle.offset;

                            if current_offset >= file_size {
                                resp.len = 0;
                                resp.status = 0; // EOF
                            } else {
                                let mut actual_len = if read_len < 128 { read_len } else { 128 };
                                if current_offset + actual_len > file_size {
                                    actual_len = file_size - current_offset;
                                }
                                unsafe {
                                    for i in 0..actual_len {
                                        resp.data[i] = FILES[file_idx].data[current_offset + i];
                                    }
                                }
                                handle.offset += actual_len;
                                resp.len = actual_len as u64;
                                resp.status = actual_len as i64;
                            }
                         } else {
                             resp.status = -9;
                         }
                     } else {
                         resp.status = -9;
                     }
                },
                FsRequest::OP_WRITE => {
                     let fd = req.arg1 as usize;
                     let write_len = req.arg2 as usize;

                     if fd < MAX_HANDLES && unsafe { HANDLES[fd].used } {
                         let handle = unsafe { &mut HANDLES[fd] };
                         let file_idx = handle.file_idx;

                         if file_idx < MAX_FILES && unsafe { FILES[file_idx].used } {
                             let current_size = unsafe { FILES[file_idx].size };
                             let current_offset = handle.offset;

                             let mut actual_len = if write_len < 128 { write_len } else { 128 };
                             if current_offset + actual_len > FILE_SIZE {
                                 actual_len = FILE_SIZE - current_offset;
                             }

                             if actual_len > 0 {
                                 unsafe {
                                     for i in 0..actual_len {
                                         FILES[file_idx].data[current_offset + i] = req.path[i];
                                     }
                                     if current_offset + actual_len > current_size {
                                         FILES[file_idx].size = current_offset + actual_len;
                                     }
                                 }
                                 handle.offset += actual_len;
                                 resp.len = actual_len as u64;
                                 resp.status = actual_len as i64;
                             } else {
                                 resp.status = 0;
                                 if write_len > 0 {
                                     resp.status = -28; // ENOSPC
                                 }
                             }
                         } else {
                             resp.status = -9;
                         }
                     } else {
                         resp.status = -9;
                     }
                },
                FsRequest::OP_CLOSE => {
                    let fd = req.arg1 as usize;
                    if fd < MAX_HANDLES && unsafe { HANDLES[fd].used } {
                        unsafe { HANDLES[fd].used = false; }
                        resp.status = 0;
                        println!("[FS] Closed FD={}", fd);
                    } else {
                        resp.status = -9;
                    }
                },
                _ => {
                    println!("[FS] Unknown OP");
                }
            }

            let resp_slice = unsafe {
                core::slice::from_raw_parts(&resp as *const _ as *const u8, size_of::<FsResponse>())
            };
            // エラーハンドリングは省略
            let _ = ipc::send(sender, resp_slice);

        } else {
            thread::yield_now();
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("PANIC in fs_service: {}", info);
    loop {
        thread::yield_now();
    }
}
