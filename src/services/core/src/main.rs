#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::{self, Write};
use swiftlib::io;
use swiftlib::task;
use swiftlib::sys::SyscallNumber;

// 簡易的な標準出力ライター
struct Stdout;
impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        io::write_stdout(s.as_bytes());
        Ok(())
    }
}

macro_rules! println {
    () => (print!("\n"));
    ($($arg:tt)*) => ({
        let _ = writeln!(&mut Stdout, $($arg)*);
    });
}

/// サービス定義
struct ServiceDef {
    name: &'static str,
    path: &'static str,
    order: u32,
}

/// 起動するサービスのリスト（index.tomlから生成する想定）
/// 現在は静的に定義
const SERVICES: &[ServiceDef] = &[
    ServiceDef {
        name: "fs.service",
        path: "fs.service",
        order: 10,
    },
    // 将来的には他のサービスも追加
    // ServiceDef { name: "net.service", path: "net.service", order: 20 },
];

/// サービスを起動する
fn start_service(service: &ServiceDef) -> Result<u64, &'static str> {
    println!("[CORE] Starting service: {} (order={})", service.name, service.order);
    
    // execシステムコールを使用してサービスを起動
    let pid = unsafe {
        let path_ptr = service.path.as_ptr() as u64;
        let path_len = service.path.len() as u64;
        let name_ptr = service.name.as_ptr() as u64;
        let name_len = service.name.len() as u64;
        
        let result: u64;
        core::arch::asm!(
            "syscall",
            in("rax") SyscallNumber::Exec as u64,
            in("rdi") path_ptr,
            in("rsi") path_len,
            in("rdx") name_ptr,
            in("r10") name_len,
            lateout("rax") result,
            options(nostack),
        );
        result
    };
    
    if pid == u64::MAX {
        println!("[CORE] Failed to start {}", service.name);
        return Err("exec failed");
    }
    
    println!("[CORE] Started {} with PID {}", service.name, pid);
    
    // サービスが初期化されるまで少し待つ
    task::sleep(100);
    
    Ok(pid)
}

#[no_mangle]
pub extern "C" fn main(_argc: i32, _argv: *const *const u8) -> i32 {
    println!("[CORE] Service Manager Started");
    println!("[CORE] Version: 0.1.0");
    
    // すべてのサービスをorder順に起動
    for service in SERVICES.iter() {
        match start_service(service) {
            Ok(pid) => {
                println!("[CORE] ✓ {} is running (PID={})", service.name, pid);
            }
            Err(e) => {
                println!("[CORE] ✗ Failed to start {}: {}", service.name, e);
            }
        }
    }
    
    println!("[CORE] All services started. Entering monitoring loop...");
    
    // サービス監視ループ
    // TODO: サービスのクラッシュ検出や再起動を実装
    loop {
        task::sleep(1000); // 1秒ごとに監視
        // TODO: サービスの状態チェック
    }
}
