use swiftlib::ipc;
use swiftlib::process;
use swiftlib::task;
use swiftlib::time;

/// READY通知OPコード
const OP_NOTIFY_READY: u64 = 0xFF;

/// サービス定義
struct ServiceDef {
    name: &'static str,
    path: &'static str,
}

const CRITICAL_SERVICES: &[ServiceDef] = &[];

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

fn start_background_service(service: &ServiceDef) -> Option<u64> {
    println!("[CORE] Starting background service: {}", service.name);
    match exec_file_via_fs_service(service.path) {
        Ok(pid) => {
            println!("[CORE] {} started (PID={})", service.name, pid);
            Some(pid)
        }
        Err(errno) => {
            println!("[CORE] exec failed for {}: errno={}, falling back", service.name, errno);
            start_service(service)
        }
    }
}

fn wait_for_ready(expected_pids: &[u64]) -> bool {
    let mut pending: Vec<u64> = expected_pids.iter().copied().filter(|pid| *pid != 0).collect();

    if pending.is_empty() {
        println!("[CORE] WARNING: no critical services to wait for");
        return true;
    }

    let total = pending.len();
    let mut recv_buf = [0u8; 64];
    let timeout = std::time::Duration::from_secs(20);
    let start = std::time::Instant::now();

    println!("[CORE] Waiting for {} critical service(s) to be ready...", total);

    while !pending.is_empty() {
        if start.elapsed() >= timeout {
            println!("[CORE] ERROR: timed out waiting for critical services");
            return false;
        }
        let (sender, len) = ipc::ipc_recv(&mut recv_buf);
        if sender == 0 && len == 0 {
            time::sleep_ms(0);
            continue;
        }

        if sender != 0 && (len as usize) >= 8 {
            // OP コードだけ読む
            let op = u64::from_le_bytes(recv_buf[..8].try_into().unwrap_or([0; 8]));
            if op == OP_NOTIFY_READY {
                if let Some(pos) = pending.iter().position(|pid| *pid == sender) {
                    pending.swap_remove(pos);
                    let ready_count = total - pending.len();
                    println!(
                        "[CORE] Critical service ready (PID={}, {}/{})",
                        sender, ready_count, total
                    );
                    if pending.is_empty() {
                        return true;
                    }
                }
            }
        }
    }

    true
}

fn exec_file_via_fs_service(path: &str) -> Result<u64, i64> {
    let fallback = path.rsplit('/').next().unwrap_or(path);
    process::exec(fallback).map_err(|_| -2)
}

fn service_name_from_path(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_allowed_service_path(path: &str) -> bool {
    if path.is_empty() || path.contains("..") {
        return false;
    }
    path.starts_with("/Services/")
        || path.starts_with("/Binaries/")
        || path.starts_with("Services/")
        || path.starts_with("Binaries/")
}

fn service_already_running(path: &str) -> bool {
    task::find_process_by_name(service_name_from_path(path)).is_some()
}

fn start_shell_service() {
    if service_already_running("shell.service") {
        println!("[CORE] shell.service already running, skip startup");
        return;
    }

    println!("[CORE] Loading shell.service...");
    match exec_file_via_fs_service("/Services/shell.service") {
        Ok(pid) => println!("[CORE] shell.service started (PID={})", pid),
        Err(errno) => {
            println!("[CORE] Failed to exec shell.service: errno={}", errno);
            println!("[CORE] Fallback: launching shell.service from initfs...");
            match process::exec("shell.service") {
                Ok(pid) => println!("[CORE] shell.service started (PID={})", pid),
                Err(_) => println!("[CORE] Failed to start shell.service"),
            }
        }
    }
}

fn fs_open_read_lines(path: &str) -> Result<Vec<String>, i64> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let mut lines = Vec::new();
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                lines.push(line.to_string());
            }
            Ok(lines)
        }
        Err(_) => Err(-2),
    }
}

fn main() {
    println!("[CORE] Service Manager Started");

    let mut critical_pids = [0u64; CRITICAL_SERVICES.len()];
    for (idx, service) in CRITICAL_SERVICES.iter().enumerate() {
        let Some(pid) = start_service(service) else {
            println!(
                "[CORE] ERROR: failed to start critical service {}, aborting startup",
                service.name
            );
            return;
        };
        critical_pids[idx] = pid;
    }

    if !wait_for_ready(&critical_pids) {
        println!("[CORE] Critical services readiness failed; aborting startup");
        return;
    }

    start_shell_service();

    // Try to read /Config/services.list and start listed services from rootfs.
    match fs_open_read_lines("/Config/services.list") {
        Ok(lines) => {
            println!("[CORE] Found services.list with {} entries", lines.len());
            for p in lines {
                if !is_allowed_service_path(&p) {
                    println!("[CORE] Skipping disallowed service path: {}", p);
                    continue;
                }
                if service_already_running(&p) {
                    println!(
                        "[CORE] Skipping {} ({} already running)",
                        p,
                        service_name_from_path(&p)
                    );
                    continue;
                }
                println!("[CORE] Requesting exec for {}", p);
                match exec_file_via_fs_service(&p) {
                    Ok(pid) => println!("[CORE] {} started (PID={})", p, pid),
                    Err(errno) => println!("[CORE] Failed to exec {}: errno={}", p, errno),
                }
            }
        }
        Err(errno) => {
            println!("[CORE] No services.list (errno={}), falling back to background list", errno);
            for service in BACKGROUND_SERVICES {
                let _ = start_background_service(service);
            }
        }
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
