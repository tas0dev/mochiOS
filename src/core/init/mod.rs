//! 起動時に実行する初期化処理をまとめたモジュール

use crate::{debug, interrupt, mem, task, util, BootInfo, MemoryRegion, Result};

pub mod fs;

pub fn kinit(boot_info: &'static BootInfo) -> Result<&'static [MemoryRegion]> {
    util::console::init();
    util::vga::init(
        boot_info.framebuffer_addr,
        boot_info.screen_width,
        boot_info.screen_height,
        boot_info.stride,
    );

    // CPU機能の初期化（SSE/FPU有効化）
    crate::cpu::init();

    let memory_map = unsafe {
        core::slice::from_raw_parts(
            boot_info.memory_map_addr as *const MemoryRegion,
            boot_info.memory_map_len,
        )
    };

    for (i, region) in memory_map.iter().enumerate() {
        debug!(
            "  Region {}: {:#x} - {:#x} ({:?})",
            i,
            region.start,
            region.start + region.len,
            region.region_type
        );
    }

    task::init_scheduler();

    // 先にフレームアロケータを初期化
    mem::init_frame_allocator(memory_map)?;

    // メモリ管理の初期化
    mem::init(boot_info);

    fs::init();

    unsafe {
        x86_64::instructions::interrupts::enable();
    }

    interrupt::init_pit();
    interrupt::enable_timer_interrupt();

    // SYSCALL/SYSRET 命令サポートを初期化
    crate::syscall::syscall_entry::init_syscall();

    Ok(memory_map)
}
