use core::mem::size_of;
use std::vec::Vec;

use swiftlib::ipc;
use swiftlib::task;
use swiftlib::time;

const OP_NOTIFY_READY: u64 = 0xFF;
const FS_PATH_MAX: usize = 128;
const FS_DATA_MAX: usize = 560;
const DRIVER_CONFIG_PATH: &str = "Config/drivers.list";
const DEFAULT_DRIVERS: &[&str] = &["Binaries/drivers/usb.elf"];

#[repr(C)]
#[derive(Clone, Copy)]
struct FsRequest {
    op: u64,
    arg1: u64,
    arg2: u64,
    path: [u8; FS_PATH_MAX],
}

impl FsRequest {
    const OP_OPEN: u64 = 1;
    const OP_READ: u64 = 2;
    const OP_CLOSE: u64 = 4;
    const OP_EXEC: u64 = 5;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FsResponse {
    status: i64,
    len: u64,
    data: [u8; FS_DATA_MAX],
}

fn fs_request(fs_tid: u64, req: &FsRequest) -> Option<FsResponse> {
    let req_slice = unsafe {
        core::slice::from_raw_parts(req as *const _ as *const u8, size_of::<FsRequest>())
    };
    if ipc::ipc_send(fs_tid, req_slice) != 0 {
        return None;
    }

    let mut resp_buf = [0u8; size_of::<FsResponse>()];
    loop {
        let (sender, len) = ipc::ipc_recv_wait(&mut resp_buf);
        if sender == 0 && len == 0 {
            continue;
        }
        if sender != fs_tid || (len as usize) < size_of::<FsResponse>() {
            continue;
        }
        let resp: FsResponse = unsafe {
            core::ptr::read_unaligned(resp_buf.as_ptr() as *const FsResponse)
        };
        return Some(resp);
    }
}

fn fs_exec(fs_tid: u64, path: &str) -> Result<u64, ()> {
    let mut path_buf = [0u8; FS_PATH_MAX];
    let bytes = path.as_bytes();
    if bytes.len() >= FS_PATH_MAX {
        return Err(());
    }
    path_buf[..bytes.len()].copy_from_slice(bytes);
    let req = FsRequest {
        op: FsRequest::OP_EXEC,
        arg1: 0,
        arg2: 0,
        path: path_buf,
    };
    let resp = fs_request(fs_tid, &req).ok_or(())?;
    if resp.status < 0 {
        return Err(());
    }
    Ok(resp.status as u64)
}

fn fs_open(fs_tid: u64, path: &str) -> Result<u64, ()> {
    let mut path_buf = [0u8; FS_PATH_MAX];
    let bytes = path.as_bytes();
    if bytes.len() >= FS_PATH_MAX {
        return Err(());
    }
    path_buf[..bytes.len()].copy_from_slice(bytes);
    let req = FsRequest {
        op: FsRequest::OP_OPEN,
        arg1: 0,
        arg2: 0,
        path: path_buf,
    };
    let resp = fs_request(fs_tid, &req).ok_or(())?;
    if resp.status < 0 {
        return Err(());
    }
    Ok(resp.status as u64)
}

fn fs_read(fs_tid: u64, fd: u64, out: &mut [u8]) -> Result<usize, ()> {
    let req = FsRequest {
        op: FsRequest::OP_READ,
        arg1: fd,
        arg2: out.len() as u64,
        path: [0u8; FS_PATH_MAX],
    };
    let resp = fs_request(fs_tid, &req).ok_or(())?;
    if resp.status < 0 {
        return Err(());
    }
    let n = resp.len as usize;
    if n > out.len() || n > FS_DATA_MAX {
        return Err(());
    }
    out[..n].copy_from_slice(&resp.data[..n]);
    Ok(n)
}

fn fs_close(fs_tid: u64, fd: u64) {
    let req = FsRequest {
        op: FsRequest::OP_CLOSE,
        arg1: fd,
        arg2: 0,
        path: [0u8; FS_PATH_MAX],
    };
    let _ = fs_request(fs_tid, &req);
}

fn load_driver_list(fs_tid: u64) -> Vec<String> {
    let mut drivers = Vec::new();

    match fs_open(fs_tid, DRIVER_CONFIG_PATH) {
        Ok(fd) => {
            let mut buf = [0u8; FS_DATA_MAX];
            let mut chunk = [0u8; FS_DATA_MAX];
            let mut text = String::new();
            loop {
                let n = match fs_read(fs_tid, fd, &mut chunk) {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if n == 0 {
                    break;
                }
                buf[..n].copy_from_slice(&chunk[..n]);
                if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                    text.push_str(s);
                }
            }
            fs_close(fs_tid, fd);
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                drivers.push(line.to_string());
            }
        }
        Err(_) => {
            println!(
                "[DRIVER] Failed to open {} via fs.service (using defaults)",
                DRIVER_CONFIG_PATH
            );
        }
    }

    if drivers.is_empty() {
        for path in DEFAULT_DRIVERS {
            drivers.push((*path).to_string());
        }
    }

    drivers
}

fn start_driver(fs_tid: u64, path: &str) {
    println!("[DRIVER] Starting {}", path);
    match fs_exec(fs_tid, path) {
        Ok(pid) => println!("[DRIVER] Started {} (PID={})", path, pid),
        Err(_) => println!("[DRIVER] Failed to start {}", path),
    }
}

fn notify_ready_to_core() {
    let core_pid = match task::find_process_by_name("core.service") {
        Some(pid) => pid,
        None => {
            println!("[DRIVER] WARNING: core.service not found, skipping READY notify");
            return;
        }
    };

    let op_bytes = OP_NOTIFY_READY.to_le_bytes();
    if ipc::ipc_send(core_pid, &op_bytes) == 0 {
        println!("[DRIVER] Sent READY to core.service (PID={})", core_pid);
    } else {
        println!("[DRIVER] Failed to send READY to core.service");
    }
}

fn main() {
    println!("[DRIVER] Driver service started");

    let fs_tid = match task::find_process_by_name("fs.service") {
        Some(pid) => pid,
        None => {
            println!("[DRIVER] fs.service not found");
            loop {
                time::sleep_ms(1000);
            }
        }
    };

    let drivers = load_driver_list(fs_tid);
    for path in &drivers {
        start_driver(fs_tid, path);
    }

    notify_ready_to_core();

    println!("[DRIVER] Entering monitoring loop...");
    loop {
        time::sleep_ms(1000);
    }
}
