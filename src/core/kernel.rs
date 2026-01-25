use crate::{init::{fs, kinit}, task, util, BootInfo, MemoryRegion, Result};
use crate::{debug, info, vprintln, sprintln};
use crate::error::handle_kernel_error;
use crate::error::{KernelError, ProcessError};

const KERNEL_THREAD_STACK_SIZE: usize = 4096 * 8;

#[repr(align(16))]
struct KernelStack([u8; KERNEL_THREAD_STACK_SIZE]);

static mut KERNEL_THREAD_STACK: KernelStack = KernelStack([0; KERNEL_THREAD_STACK_SIZE]);

/// カーネルエントリーポイント
#[no_mangle]
pub extern "C" fn kernel_entry(boot_info: &'static BootInfo) -> ! {
    util::log::set_level(util::log::LogLevel::Info);
    let memory_map = match kinit(boot_info) {
        Ok(map) => map,
        Err(e) => {
            handle_kernel_error(e);
            halt_forever();
        }
    };

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
    let kernel_process = task::Process::new("swiftcore", task::PrivilegeLevel::Core, None, 0);
    let kernel_pid = kernel_process.id();

    if task::add_process(kernel_process).is_none() {
        return Err(KernelError::Process(ProcessError::MaxProcessesReached));
    }

    let stack_addr = unsafe { core::ptr::addr_of!(KERNEL_THREAD_STACK.0) as *const u8 as u64 };
    let kernel_thread = task::Thread::new(
        kernel_pid,
        "core",
        kernel_idle,
        stack_addr,
        KERNEL_THREAD_STACK_SIZE,
    );

    if task::add_thread(kernel_thread).is_none() {
        return Err(KernelError::Process(ProcessError::MaxProcessesReached));
    }

    // NOTE:
    // The kernel no longer auto-starts services or the scheduler.
    // A userland service (e.g. `core.service`) will act as the service
    // manager and enable multitasking/scheduling as needed.
    info!("kernel running in single-task mode (no auto-start services)");

    #[allow(unreachable_code)]
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
