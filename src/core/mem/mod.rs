//! メモリ管理モジュール
//!
//! GDT、TSS、ページング、フレームアロケータ

use crate::{debug, info, interrupt, sprintln, MemoryRegion, Result};

pub mod frame;
pub mod gdt;
pub mod paging;
pub mod tss;

pub fn init(boot_info: &'static crate::BootInfo) {
    info!("Initializing memory...");

    x86_64::instructions::interrupts::disable();

    gdt::init();
    interrupt::init_idt();

    paging::init(boot_info);

    // PITを停止してからPICを初期化
    interrupt::disable_pit();
    interrupt::init_pic();

    debug!("Memory initialized");
}

/// メモリマップを設定してフレームアロケータを初期化
pub fn init_frame_allocator(memory_map: &'static [MemoryRegion]) -> Result<()> {
    frame::init(memory_map);

    if let Some((total, frames)) = frame::get_memory_info() {
        debug!(
            "Physical memory: {} MB ({} frames)",
            total / 1024 / 1024,
            frames
        );
    }

    Ok(())
}
