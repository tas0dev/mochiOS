use swiftlib::ipc;
use swiftlib::process;
use swiftlib::time;

/// READY通知 OP コード（disk.service / fs.service が送信）
const OP_NOTIFY_READY: u64 = 0xFF;

/// サービス定義
struct ServiceDef {
    name: &'static str,
    path: &'static str,
}

const SERVICES: &[ServiceDef] = &[
    ServiceDef { name: "disk.service", path: "disk.service" },
    ServiceDef { name: "fs.service",   path: "fs.service"   },
];

#[cfg(feature = "run_tests")]
const TEST_PATH: &str = "tests";

fn start_service(service: &ServiceDef) {
    println!("[CORE] Starting service: {}", service.name);
    match process::exec(service.path) {
        Ok(pid) => {
            println!("[CORE] {} started (PID={})", service.name, pid);
            time::sleep_ms(100);
        }
        Err(_) => println!("[CORE] Failed to start {}", service.name),
    }
}

/// disk.service と fs.service の READY 通知を待つ（最大タイムアウト付き）
fn wait_for_ready(expected_count: usize) {
    let mut ready_count = 0;
    let mut recv_buf = [0u8; 64];

    println!("[CORE] Waiting for {} service(s) to be ready...", expected_count);

    // 最大 30 秒待つ（タイムアウト: 300 × 100ms）
    for _ in 0..300 {
        let (sender, len) = ipc::ipc_recv(&mut recv_buf);

        if sender == 0xFFFFFFFF || len == 0xFFFFFFFD {
            time::sleep_ms(100);
            continue;
        }

        if sender != 0 && (len as usize) >= 8 {
            // OP コードだけ読む
            let op = u64::from_le_bytes(recv_buf[..8].try_into().unwrap_or([0; 8]));
            if op == OP_NOTIFY_READY {
                ready_count += 1;
                println!("[CORE] Service ready (PID={}, {}/{})", sender, ready_count, expected_count);
                if ready_count >= expected_count {
                    return;
                }
            }
        }
    }

    println!("[CORE] WARNING: Timed out waiting for READY (got {}/{})", ready_count, expected_count);
}

fn main() {
    println!("[CORE] Service Manager Started");

    for service in SERVICES {
        start_service(service);
    }

    // disk と fs が揃うまで待つ
    wait_for_ready(2);

    // shell.service を起動（std::fs::read で ELF を読んで exec_from_buffer で実行）
    println!("[CORE] Loading shell.service from Services/...");
    match std::fs::read("Services/shell.service") {
        Ok(elf_data) => {
            println!("[CORE] shell.service loaded ({} bytes), launching...", elf_data.len());
            match process::exec_from_buffer(&elf_data) {
                Ok(pid) => println!("[CORE] shell.service started (PID={})", pid),
                Err(_)  => println!("[CORE] Failed to exec shell.service"),
            }
        }
        Err(e) => println!("[CORE] Failed to read shell.service: {}", e),
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
