//! カーネルエントリーポイント

use crate::result::handle_kernel_error;
use crate::result::{Kernel, Process};
use crate::{info, sprintln, vprintln};
use crate::{init::kinit, task, util, BootInfo, MemoryRegion, Result};

const KERNEL_THREAD_STACK_SIZE: usize = 4096 * 8;

#[repr(align(16))]
struct KernelStack([u8; KERNEL_THREAD_STACK_SIZE]);

static mut KERNEL_THREAD_STACK: KernelStack = KernelStack([0; KERNEL_THREAD_STACK_SIZE]);

/// カーネル初期化いろいろ（エントリーポイント）
#[no_mangle]
pub extern "C" fn kernel_entry(boot_info: &'static BootInfo) -> ! {
    util::log::set_level(util::log::LogLevel::Debug);
    let memory_map = match kinit(boot_info) {
        Ok(map) => map,
        Err(e) => {
            handle_kernel_error(e);
            halt_forever();
        }
    };

    create_kernel_proc().unwrap_or_else(|e| {
        handle_kernel_error(e);
        halt_forever();
    });

    match kernel_main(boot_info, memory_map) {
        Ok(_) => {
            info!("Kernel shutdown gracefully");
            halt_forever();
        }
        Err(e) => {
            handle_kernel_error(e);
            halt_forever();
        }
    }
}

/// カーネルメイン処理
fn kernel_main(boot_info: &'static BootInfo, memory_map: &'static [MemoryRegion]) -> Result<()> {
    match crate::syscall::exec::exec_kernel(
        crate::init::fs::read("/hello.bin").map(|_| 0).unwrap_or(0),
    ) {
        r => {
            crate::debug!("exec returned: {}", r);
        }
    }

    info!("Starting task scheduler...");
    task::start_scheduling();

    #[allow(unreachable_code)]
    Ok(())
}

fn create_kernel_proc() -> Result<()> {
    let kernel_process = task::Process::new("kernel", task::PrivilegeLevel::Core, None, 0);
    let kernel_pid = kernel_process.id();

    if task::add_process(kernel_process).is_none() {
        return Err(Kernel::Process(Process::MaxProcessesReached));
    }

    let stack_addr =
        unsafe { (&raw const KERNEL_THREAD_STACK as *const KernelStack as *const u8) as u64 };
    let kernel_thread = task::Thread::new(
        kernel_pid,
        "kernel-idle",
        kernel_idle,
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

fn kernel_idle() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
