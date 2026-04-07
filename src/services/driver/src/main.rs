use std::vec::Vec;

use swiftlib::time;
use swiftlib::task;
use swiftlib::fs;
use swiftlib::ipc;

const OP_NOTIFY_READY: u64 = 0xFF;
const DRIVER_CONFIG_PATH: &str = "Config/drivers.list";
const DEFAULT_DRIVERS: &[&str] = &["Binaries/drivers/usb.elf"];

fn load_driver_list(_fs_tid: u64) -> Vec<String> {
    let mut drivers = Vec::new();

    match swiftlib::fs::read_file_via_fs(DRIVER_CONFIG_PATH, 4096) {
        Ok(Some(bytes)) => {
            match core::str::from_utf8(&bytes) {
                Ok(text) => {
                    for line in text.lines() {
                        let line = line.trim();
                        if line.is_empty() || line.starts_with('#') {
                            continue;
                        }
                        drivers.push(line.to_string());
                    }
                }
                Err(e) => {
                    println!(
                        "[DRIVER] Invalid UTF-8 in {} (len={}): {}. Using defaults.",
                        DRIVER_CONFIG_PATH,
                        bytes.len(),
                        e
                    );
                }
            }
        }
        Ok(None) => {
            println!(
                "[DRIVER] Failed to open {} via fs.service (using defaults)",
                DRIVER_CONFIG_PATH
            );
        }
        Err(errno) => {
            println!(
                "[DRIVER] Failed to read {} via fs.service: errno={} (using defaults)",
                DRIVER_CONFIG_PATH,
                errno
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
    let _ = fs_tid;
    match fs::exec_via_fs(path) {
        Ok(pid) => println!("[DRIVER] Started {} (PID={})", path, pid),
        Err(errno) => println!("[DRIVER] Failed to start {} (errno={})", path, errno),
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

    const MAX_ATTEMPTS: usize = 30;
    const RETRY_MS: u64 = 500;
    let mut fs_tid = None;
    for attempt in 1..=MAX_ATTEMPTS {
        fs_tid = task::find_process_by_name("fs.service");
        if fs_tid.is_some() {
            break;
        }
        println!(
            "[DRIVER] fs.service not found (attempt {}/{}), retrying in {}ms",
            attempt, MAX_ATTEMPTS, RETRY_MS
        );
        time::sleep_ms(RETRY_MS);
    }
    let fs_tid = match fs_tid {
        Some(pid) => pid,
        None => {
            println!(
                "[DRIVER] ERROR: fs.service not found after {} attempts, exiting",
                MAX_ATTEMPTS
            );
            std::process::exit(1);
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
