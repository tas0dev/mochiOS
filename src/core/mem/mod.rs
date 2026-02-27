//! メモリ管理モジュール
//!
//! GDT、TSS、ページング、フレームアロケータ

use crate::{debug, info, interrupt, sprintln, MemoryRegion, Result};

pub mod frame;
pub mod gdt;
pub mod paging;
pub mod tss;
pub mod allocator;

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

    allocator::init_heap(
        &mut *paging::PAGE_TABLE.lock().as_mut().expect("PAGE_TABLE not initialized"),
        &mut *frame::FRAME_ALLOCATOR.lock().as_mut().expect("FRAME_ALLOCATOR not initialized"),
        boot_info.kernel_heap_addr,
    ).expect("Heap initialization failed");

    // カーネルアロケータへ切り替え
    unsafe {
       let ptr = boot_info.allocator_addr as *mut core::sync::atomic::AtomicBool;
       (*ptr).store(true, core::sync::atomic::Ordering::Relaxed);
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
