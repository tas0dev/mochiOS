use crate::result::handle_kernel_error;
use crate::result::{Kernel, Process};
use crate::{debug, info, sprintln, vprintln};
use crate::{init::kinit, task, util, BootInfo, MemoryRegion, Result};
use crate::init::fs::{read, entries};
use crate::syscall::exec::{exec_kernel, exec_kernel_with_name};
use crate::util::log::LogLevel;

const KERNEL_THREAD_STACK_SIZE: usize = 4096 * 8;

#[repr(align(16))]
struct KernelStack([u8; KERNEL_THREAD_STACK_SIZE]);

static mut KERNEL_THREAD_STACK: KernelStack = KernelStack([0; KERNEL_THREAD_STACK_SIZE]);

/// カーネルメイン関数
fn kernel_main() -> ! {
    util::log::set_level(LogLevel::Info);
    debug!("Kernel started");

    // .service ファイルを自動実行
    for entry in entries() {
        if entry.name.ends_with(".service") {
            // パス文字列の準備
            let path = entry.name;

            // サービス名（ドメイン）のマッピング
            let service_name = match path {
                "initfs.service" => "core.service.initfs",
                "test_service.service" => "ext.service.test",
                _ => path,
            };

            info!("Starting service: {} as {}", path, service_name);

            // exec_kernel_with_name を使用
            exec_kernel_with_name(path, service_name);
        }
    }

    let test_elf_path = "test_app.elf\0";
    exec_kernel(test_elf_path.as_ptr() as u64);

    loop {
        x86_64::instructions::hlt();
    }
}

/// カーネルエントリポイント
#[no_mangle]
pub extern "C" fn kernel_entry(boot_info: &'static BootInfo) -> ! {
    let memory_map = match kinit(boot_info) {
        Ok(map) => map,
        Err(e) => {
            handle_kernel_error(e);
            halt_forever();
        }
    };

    create_kernel_proc(boot_info, memory_map).unwrap_or_else(|e| {
        handle_kernel_error(e);
        halt_forever();
    });
    task::start_scheduling();
}

/// カーネルメインプロセスの作成
fn create_kernel_proc(boot_info: &'static BootInfo, memory_map: &'static [MemoryRegion]) -> Result<()> {
    let kernel_process = task::Process::new("kernel", task::PrivilegeLevel::Core, None, 0);
    let kernel_pid = kernel_process.id();

    if task::add_process(kernel_process).is_none() {
        return Err(Kernel::Process(Process::MaxProcessesReached));
    }

    let stack_addr = unsafe { (&raw const KERNEL_THREAD_STACK as *const u8) as u64 };
    let kernel_thread = task::Thread::new(
        kernel_pid,
        "core",
        kernel_main,
        stack_addr,
        KERNEL_THREAD_STACK_SIZE,
    );

    if task::add_thread(kernel_thread).is_none() {
        return Err(Kernel::Process(Process::MaxProcessesReached));
    }

    Ok(())
}

/// システムを無限ループで停止
fn halt_forever() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}