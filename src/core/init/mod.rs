//! 起動時に実行する初期化処理をまとめたモジュール

use crate::{
    debug, driver, interrupt, mem, task, util, BootInfo, MemoryRegion, Result,
};

pub mod fs;

pub fn kinit(boot_info: &'static BootInfo) -> Result<&'static [MemoryRegion]> {
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
        debug!(
            "  Region {}: {:#x} - {:#x} ({:?})",
            i,
            region.start,
            region.start + region.len,
            region.region_type
        );
    }

    driver::ps2_keyboard::init();

    mem::init(boot_info.physical_memory_offset);
    mem::init_frame_allocator(memory_map)?;

    unsafe {
        x86_64::instructions::interrupts::enable();
    }

    // Initialize syscall MSRs (STAR/LSTAR/FMASK)
    interrupt::init_syscall();

    interrupt::init_pit();
    // Timer interrupts are not enabled by default. Userland `core.service`
    // will manage multitasking and enable scheduling if desired.
    // interrupt::enable_timer_interrupt();

    Ok(memory_map)
}