//! メモリ管理モジュール
//!
//! GDT、TSS、ページング、フレームアロケータ

use crate::{debug, info, interrupt, sprintln, MemoryRegion, Result};

pub mod allocator;
pub mod frame;
pub mod gdt;
pub mod paging;
pub mod tss;
mod user;

/// メモリの初期化
///
/// ## Arguments
/// - `boot_info`: ブートローダから渡される情報構造体
pub fn init(boot_info: &'static crate::BootInfo) {
    info!("Initializing memory...");

    x86_64::instructions::interrupts::disable();

    gdt::init();
    interrupt::init_idt();

    paging::init(boot_info);

    let mut page_table_lock = paging::PAGE_TABLE.lock();
    let page_table = match page_table_lock.as_mut() {
        Some(p) => p,
        None => {
            crate::warn!("PAGE_TABLE not initialized");
            loop {
                x86_64::instructions::hlt();
            }
        }
    };
    let mut frame_alloc_lock = frame::FRAME_ALLOCATOR.lock();
    let frame_alloc = match frame_alloc_lock.as_mut() {
        Some(fa) => fa,
        None => {
            crate::warn!("FRAME_ALLOCATOR not initialized");
            loop {
                x86_64::instructions::hlt();
            }
        }
    };
    if let Err(e) = allocator::init_heap(
        &mut *page_table,
        &mut *frame_alloc,
        boot_info.kernel_heap_addr,
    ) {
        crate::warn!("Heap initialization failed: {:?}", e);
        loop {
            x86_64::instructions::hlt();
        }
    }

    // PITを停止してからPICを初期化
    interrupt::disable_pit();
    interrupt::init_pic();

    debug!("Memory initialized");
}

/// メモリマップを設定してフレームアロケータを初期化
///
/// ## Arguments
/// - `memory_map`: ブートローダから渡されるメモリマップ
///
/// ## Returns
/// - `Result<()>`: 成功すればOk、失敗すればErr
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
