use crate::result::handle_kernel_error;
use crate::result::{Kernel, Process};
use crate::{debug, info, sprintln, vprintln};
use crate::{init::kinit, task, util, BootInfo, MemoryRegion, Result};
use crate::syscall::exec::exec_kernel_with_name;
use crate::util::log::LogLevel;

const KERNEL_THREAD_STACK_SIZE: usize = 4096 * 8;

#[repr(align(16))]
struct KernelStack([u8; KERNEL_THREAD_STACK_SIZE]);

static mut KERNEL_THREAD_STACK: KernelStack = KernelStack([0; KERNEL_THREAD_STACK_SIZE]);

/// カーネルメイン関数
fn kernel_main() -> ! {
    util::log::set_level(LogLevel::Debug);
    debug!("Kernel started");

    // core.serviceのみ起動（他のサービスはcore.serviceが管理）
    info!("Starting core.service");
    exec_kernel_with_name("core.service", "core.service");

    // カーネルはアイドル状態に入る
    info!("Kernel initialization complete. Entering idle loop...");
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