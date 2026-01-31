//! カーネルエントリーポイント

use crate::error::handle_kernel_error;
use crate::error::{KernelError, ProcessError};
use crate::{info, vprintln};
use crate::{init::kinit, task, util, BootInfo, MemoryRegion, Result};

const KERNEL_THREAD_STACK_SIZE: usize = 4096 * 8;

#[repr(align(16))]
struct KernelStack([u8; KERNEL_THREAD_STACK_SIZE]);

static mut KERNEL_THREAD_STACK: KernelStack = KernelStack([0; KERNEL_THREAD_STACK_SIZE]);

/// カーネルエントリーポイント
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
    info!("Initializing kernel...");
    info!("Memory map entries: {}", boot_info.memory_map_len);

    vprintln!("Framebuffer: {:#x}", boot_info.framebuffer_addr);
    vprintln!(
        "Resolution: {}x{}",
        boot_info.screen_width,
        boot_info.screen_height
    );

    info!(
        "Physical memory offset: {:#x}",
        boot_info.physical_memory_offset
    );

    let kernel_process = task::Process::new("kernel", task::PrivilegeLevel::Core, None, 0);
    let kernel_pid = kernel_process.id();

    if task::add_process(kernel_process).is_none() {
        return Err(KernelError::Process(ProcessError::MaxProcessesReached));
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
        return Err(KernelError::Process(ProcessError::MaxProcessesReached));
    }

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
