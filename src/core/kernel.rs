//! カーネルエントリーポイント

use crate::{interrupt, mem, task, util, BootInfo, KernelError, MemoryRegion, Result};

/// カーネルエントリーポイント
#[no_mangle]
pub extern "C" fn kernel_entry(boot_info: &'static BootInfo) -> ! {
    util::log::set_level(util::log::LogLevel::Info);
    util::console::init();
    util::vga::init(
        boot_info.framebuffer_addr,
        boot_info.screen_width,
        boot_info.screen_height,
        boot_info.stride,
    );

    let memory_map = unsafe {
        core::slice::from_raw_parts(
            boot_info.memory_map_addr as *const MemoryRegion,
            boot_info.memory_map_len,
        )
    };

    for (i, region) in memory_map.iter().enumerate() {
        crate::debug!(
            "  Region {}: {:#x} - {:#x} ({:?})",
            i,
            region.start,
            region.start + region.len,
            region.region_type
        );
    }

    match kernel_main(boot_info, memory_map) {
        Ok(_) => {
            crate::info!("Kernel shutdown gracefully");
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
    crate::info!("Initializing kernel...");
    crate::info!("Memory map entries: {}", boot_info.memory_map_len);

    crate::vprintln!("Framebuffer: {:#x}", boot_info.framebuffer_addr);
    crate::vprintln!(
        "Resolution: {}x{}",
        boot_info.screen_width,
        boot_info.screen_height
    );

    crate::info!(
        "Physical memory offset: {:#x}",
        boot_info.physical_memory_offset
    );

    // メモリ管理初期化
    mem::init(boot_info.physical_memory_offset);
    mem::init_frame_allocator(memory_map)?;

    crate::info!("Kernel ready");

    // タスクシステムを初期化
    init_tasks();

    // 割込みを有効化
    crate::debug!("Enabling interrupts...");
    unsafe {
        x86_64::instructions::interrupts::enable();
    }

    // タイマー割り込みを設定（10ms周期）
    interrupt::init_pit();
    interrupt::enable_timer_interrupt();

    crate::info!("Timer interrupt configured (10ms period)");

    // スケジューリングを開始（戻ってこない）
    crate::info!("Starting task scheduler...");
    task::start_scheduling();

    #[allow(unreachable_code)]
    Ok(())
}

/// タスクシステムを初期化
fn init_tasks() {
    crate::info!("Initializing task system...");

    // スケジューラを初期化
    task::init_scheduler();

    // プロセスAを作成
    let process_a = task::Process::new(
        "Process A",
        task::PrivilegeLevel::Core,
        None, // 親プロセスなし
        0,    // 優先度
    );
    let process_a_id = task::add_process(process_a).expect("Failed to create process A");

    // プロセスBを作成
    let process_b = task::Process::new(
        "Process B",
        task::PrivilegeLevel::Core,
        None, // 親プロセスなし
        0,    // 優先度
    );
    let process_b_id = task::add_process(process_b).expect("Failed to create process B");

    // タスクA用のスタックを確保（8KB）
    const STACK_SIZE: usize = 8192;
    static mut STACK_A: [u8; STACK_SIZE] = [0; STACK_SIZE];
    static mut STACK_B: [u8; STACK_SIZE] = [0; STACK_SIZE];

    let stack_a_addr = unsafe { core::ptr::addr_of!(STACK_A) as u64 };
    let stack_b_addr = unsafe { core::ptr::addr_of!(STACK_B) as u64 };

    crate::debug!(
        "Stack A: {:#x} - {:#x}",
        stack_a_addr,
        stack_a_addr + STACK_SIZE as u64
    );
    crate::debug!(
        "Stack B: {:#x} - {:#x}",
        stack_b_addr,
        stack_b_addr + STACK_SIZE as u64
    );
    crate::debug!("Task A entry: {:p}", task_a_entry as *const ());
    crate::debug!("Task B entry: {:p}", task_b_entry as *const ());

    // スレッドAを作成
    let thread_a = task::Thread::new(
        process_a_id,
        "Thread A",
        task_a_entry,
        stack_a_addr,
        STACK_SIZE,
    );
    task::add_thread(thread_a).expect("Failed to create thread A");

    // スレッドBを作成
    let thread_b = task::Thread::new(
        process_b_id,
        "Thread B",
        task_b_entry,
        stack_b_addr,
        STACK_SIZE,
    );
    task::add_thread(thread_b).expect("Failed to create thread B");

    crate::info!("Task system initialized");
    crate::info!("  Processes: {}", task::process_count());
    crate::info!("  Threads: {}", task::thread_count());
}

/// タスクAのエントリーポイント
fn task_a_entry() -> ! {
    let mut counter = 0u64;
    loop {
        crate::vprintln!("Hello from Task A ({})", counter);
        crate::sprintln!("Hello from Task A ({})", counter);
        counter += 1;

        // 少し待機
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }
    }
}

/// タスクBのエントリーポイント
fn task_b_entry() -> ! {
    let mut counter = 0u64;
    loop {
        crate::vprintln!("Hello from Task B ({})", counter);
        crate::sprintln!("Hello from Task B ({})", counter);
        counter += 1;

        // 少し待機
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }
    }
}

/// カーネルエラーを処理
fn handle_kernel_error(error: KernelError) {
    use crate::error::*;

    crate::warn!("KERNEL ERROR: {}", error);
    crate::debug!("Is fatal: {}", error.is_fatal());
    crate::debug!("Is retryable: {}", error.is_retryable());

    match error {
        KernelError::Memory(mem_err) => {
            crate::error!("Memory error: {:?}", mem_err);
        }
        KernelError::Process(proc_err) => {
            crate::error!("Process error: {:?}", proc_err);
        }
        KernelError::Device(dev_err) => {
            crate::error!("Device error: {:?}", dev_err);
        }
        _ => {
            crate::error!("Unknown error: {:?}", error);
        }
    }

    crate::info!("System halted.");
}

/// システムを無限ループで停止
fn halt_forever() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
