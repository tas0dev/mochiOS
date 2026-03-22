use core::mem::size_of;
use swiftlib::ipc;
use swiftlib::process;
use swiftlib::task;
use swiftlib::time;

/// READY通知OPコード
const OP_NOTIFY_READY: u64 = 0xFF;
const FS_PATH_MAX: usize = 128;
const FS_DATA_MAX: usize = 560;

#[repr(C)]
#[derive(Clone, Copy)]
struct FsRequest {
    op: u64,
    arg1: u64,
    arg2: u64,
    path: [u8; FS_PATH_MAX],
}

impl FsRequest {
    const OP_EXEC: u64 = 5;

    fn exec(path: &str) -> Option<Self> {
        let mut path_buf = [0u8; FS_PATH_MAX];
        let bytes = path.as_bytes();
        if bytes.len() >= FS_PATH_MAX {
            return None;
        }
        path_buf[..bytes.len()].copy_from_slice(bytes);
        Some(Self {
            op: Self::OP_EXEC,
            arg1: 0,
            arg2: 0,
            path: path_buf,
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FsResponse {
    status: i64,
    len: u64,
    data: [u8; FS_DATA_MAX],
}

/// サービス定義
struct ServiceDef {
    name: &'static str,
    path: &'static str,
}

const CRITICAL_SERVICES: &[ServiceDef] = &[
    ServiceDef { name: "disk.service",   path: "disk.service"   },
    ServiceDef { name: "fs.service",     path: "fs.service"     },
];

const BACKGROUND_SERVICES: &[ServiceDef] = &[
    ServiceDef { name: "driver.service", path: "driver.service" },
];

#[cfg(feature = "run_tests")]
const TEST_PATH: &str = "tests";

fn start_service(service: &ServiceDef) -> Option<u64> {
    println!("[CORE] Starting service: {}", service.name);
    match process::exec(service.path) {
        Ok(pid) => {
            println!("[CORE] {} started (PID={})", service.name, pid);
            Some(pid)
        }
        Err(_) => {
            println!("[CORE] Failed to start {}", service.name);
            None
        }
    }
}

fn wait_for_ready(expected_pids: &[u64]) {
    let mut pending = [0u64; CRITICAL_SERVICES.len()];
    let mut pending_len = 0usize;
    for &pid in expected_pids {
        if pid != 0 && pending_len < pending.len() {
            pending[pending_len] = pid;
            pending_len += 1;
        }
    }

    if pending_len == 0 {
        println!("[CORE] WARNING: no critical services to wait for");
        return;
    }

    let total = pending_len;
    let mut recv_buf = [0u8; 64];

    println!("[CORE] Waiting for {} critical service(s) to be ready...", total);

    while pending_len > 0 {
        let (sender, len) = ipc::ipc_recv_wait(&mut recv_buf);
        if sender == 0 && len == 0 {
            continue;
        }

        if sender != 0 && (len as usize) >= 8 {
            // OP コードだけ読む
            let op = u64::from_le_bytes(recv_buf[..8].try_into().unwrap_or([0; 8]));
            if op == OP_NOTIFY_READY {
                let mut matched = false;
                for i in 0..pending_len {
                    if pending[i] == sender {
                        pending[i] = pending[pending_len - 1];
                        pending_len -= 1;
                        matched = true;
                        break;
                    }
                }
                if matched {
                    let ready_count = total - pending_len;
                    println!(
                        "[CORE] Critical service ready (PID={}, {}/{})",
                        sender, ready_count, total
                    );
                    if pending_len == 0 {
                        return;
                    }
                }
            }
        }
    }
}

fn fs_request(fs_pid: u64, req: &FsRequest) -> Result<FsResponse, &'static str> {
    let req_slice = unsafe {
        core::slice::from_raw_parts(req as *const _ as *const u8, size_of::<FsRequest>())
    };
    if ipc::ipc_send(fs_pid, req_slice) != 0 {
        return Err("ipc_send failed");
    }

    let mut resp_buf = [0u8; size_of::<FsResponse>()];
    loop {
        let (sender, len) = ipc::ipc_recv_wait(&mut resp_buf);
        if sender == 0 && len == 0 {
            continue;
        }
        if sender != fs_pid || (len as usize) < size_of::<FsResponse>() {
            continue;
        }

        let resp: FsResponse = unsafe {
            core::ptr::read_unaligned(resp_buf.as_ptr() as *const FsResponse)
        };
        return Ok(resp);
    }
}

fn exec_file_via_fs_service(path: &str) -> Result<u64, i64> {
    let fs_tid = task::find_process_by_name("fs.service")
        .ok_or(-3)?; // ESRCH
    let exec_req = FsRequest::exec(path).ok_or(-22)?; // EINVAL
    let resp = fs_request(fs_tid, &exec_req).map_err(|_| -5)?; // EIO
    if resp.status < 0 {
        return Err(resp.status);
    }
    Ok(resp.status as u64)
}

fn start_shell_service() {
    // rootfs は fs.service がマウントするため、fs.service に実行を依頼する
    println!("[CORE] Loading shell.service via fs.service...");
    match exec_file_via_fs_service("Services/shell.service") {
        Ok(pid) => println!("[CORE] shell.service started (PID={})", pid),
        Err(errno) => {
            println!(
                "[CORE] Failed to exec shell.service via fs.service: errno={}",
                errno
            );
            println!("[CORE] Fallback: launching shell.service from initfs...");
            match process::exec("shell.service") {
                Ok(pid) => println!("[CORE] shell.service started (PID={})", pid),
                Err(_) => println!("[CORE] Failed to start shell.service"),
            }
        }
    }
}

fn main() {
    println!("[CORE] Service Manager Started");

    let mut critical_pids = [0u64; CRITICAL_SERVICES.len()];
    for (idx, service) in CRITICAL_SERVICES.iter().enumerate() {
        critical_pids[idx] = start_service(service).unwrap_or(0);
    }

    wait_for_ready(&critical_pids);

    start_shell_service();

    for service in BACKGROUND_SERVICES {
        let _ = start_service(service);
    }

    #[cfg(feature = "run_tests")]
    {
        println!("[CORE] Starting test application...");
        match process::exec(TEST_PATH) {
            Ok(pid) => println!("[CORE] tests started (PID={})", pid),
            Err(_) => println!("[CORE] Failed to start tests"),
        }
        time::sleep_ms(100);
    }

    println!("[CORE] Entering monitoring loop...");
    loop {
        time::sleep_ms(1000);
    }
}
