//! メモリ管理モジュール
//!
//! GDT、TSS、ページング、フレームアロケータ

use crate::{debug, info, interrupt, MemoryRegion, Result};

pub mod allocator;
pub mod frame;
pub mod gdt;
pub mod paging;
pub mod tss;
pub(crate) mod user;

/// メモリの初期化
///
/// ## Arguments
/// - `boot_info`: ブートローダから渡される情報構造体
pub fn init(boot_info: &'static crate::BootInfo) -> Result<()> {
    info!("Initializing memory...");
    crate::debug!("About to disable interrupts");

    x86_64::instructions::interrupts::disable();
    crate::debug!("Interrupts disabled, temporarily disabling SMAP");

    let smap_was_enabled = crate::cpu::is_smap_enabled();
    if smap_was_enabled {
        unsafe {
            crate::cpu::disable_smap();
        }
        crate::debug!("SMAP temporarily disabled");
    }

    gdt::init();
    crate::debug!("GDT initialized, initializing IDT");
    interrupt::init_idt();
    crate::debug!("IDT initialized, initializing paging");

    paging::init(boot_info)?;
    crate::debug!("Paging initialized, initializing PAGE_TABLE");

    let smap_was_enabled_for_paging = crate::cpu::is_smap_enabled();
    let smep_was_enabled_for_paging = unsafe {
        let cr4 = x86_64::registers::control::Cr4::read();
        cr4.contains(x86_64::registers::control::Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION)
    };

    if smap_was_enabled_for_paging {
        unsafe {
            crate::cpu::disable_smap();
        }
        crate::debug!("SMAP temporarily disabled for PAGE_TABLE init");
    }
    if smep_was_enabled_for_paging {
        unsafe {
            let mut cr4 = x86_64::registers::control::Cr4::read();
            cr4.remove(x86_64::registers::control::Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION);
            x86_64::registers::control::Cr4::write(cr4);
        }
        crate::debug!("SMEP temporarily disabled for PAGE_TABLE init");
    }

    paging::init_page_table()?;
    crate::debug!("PAGE_TABLE initialized");

    // Keep SMAP/SMEP disabled during heap initialization
    crate::info!("Keeping SMAP/SMEP disabled during kernel initialization");

    let mut page_table_lock = paging::PAGE_TABLE.lock();
    let page_table = match page_table_lock.as_mut() {
        Some(p) => p,
        None => {
            crate::warn!("PAGE_TABLE not initialized");
            crate::audit::log(
                crate::audit::AuditEventKind::Fault,
                "memory init missing page table",
            );
            return Err(crate::Kernel::Memory(crate::result::Memory::NotMapped));
        }
    };
    let mut frame_alloc_lock = frame::FRAME_ALLOCATOR.lock();
    let frame_alloc = match frame_alloc_lock.as_mut() {
        Some(fa) => fa,
        None => {
            crate::warn!("FRAME_ALLOCATOR not initialized");
            crate::audit::log(
                crate::audit::AuditEventKind::Fault,
                "memory init missing frame allocator",
            );
            return Err(crate::Kernel::Memory(crate::result::Memory::OutOfMemory));
        }
    };
    crate::debug!("Locks acquired, initializing heap");
    if let Err(e) = allocator::init_heap(
        &mut *page_table,
        &mut *frame_alloc,
        boot_info.kernel_heap_addr,
    ) {
        crate::warn!("Heap initialization failed: {:?}", e);
        crate::audit::log(
            crate::audit::AuditEventKind::Fault,
            "memory init heap initialization failed",
        );
        return Err(crate::Kernel::Memory(crate::result::Memory::InvalidAddress));
    }

    crate::debug!("Heap initialized, disabling PIT");
    // PITを停止してからPICを初期化
    interrupt::disable_pit();
    crate::debug!("PIT disabled, initializing PIC");
    interrupt::init_pic();

    debug!("Memory initialized");
    Ok(())
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
