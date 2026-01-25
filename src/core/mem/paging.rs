//! ページング管理モジュール
//!
//! 仮想メモリとページテーブル管理

use crate::error::{KernelError, MemoryError, Result};
use crate::sprintln;
use spin::Mutex;
use x86_64::{
    registers::control::{Cr0, Cr0Flags},
    structures::paging::{
        mapper::MapToError, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame,
        Size4KiB,
    },
    VirtAddr,
};
use core::sync::atomic::{AtomicU64, Ordering};

static PAGE_TABLE: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
static PHYSICAL_MEMORY_OFFSET: AtomicU64 = AtomicU64::new(0);

/// ページングシステムを初期化
pub fn init(physical_memory_offset: u64) {
    sprintln!("Initializing paging...");

    unsafe {
        let level_4_table = active_level_4_table(physical_memory_offset);
        let page_table = OffsetPageTable::new(level_4_table, VirtAddr::new(physical_memory_offset));
        *PAGE_TABLE.lock() = Some(page_table);
        PHYSICAL_MEMORY_OFFSET.store(physical_memory_offset, Ordering::Relaxed);
    }

    sprintln!("Paging initialized");
}

/// 現在設定されている物理メモリオフセットを返す
pub fn physical_memory_offset() -> u64 {
    PHYSICAL_MEMORY_OFFSET.load(Ordering::Relaxed)
}

/// アクティブなレベル4ページテーブルへの参照を取得
unsafe fn active_level_4_table(physical_memory_offset: u64) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();
    let phys = level_4_table_frame.start_address();
    let virt = VirtAddr::new(phys.as_u64() + physical_memory_offset);
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr
}

/// ページをマップ
pub fn map_page(page: Page, frame: PhysFrame, flags: PageTableFlags) -> Result<()> {
    let mut page_table_lock = PAGE_TABLE.lock();
    let page_table = page_table_lock
        .as_mut()
        .ok_or(KernelError::Memory(MemoryError::NotMapped))?;

    let mut allocator_lock = super::frame::FRAME_ALLOCATOR.lock();
    let allocator = allocator_lock
        .as_mut()
        .ok_or(KernelError::Memory(MemoryError::OutOfMemory))?;

    let cr0 = Cr0::read();
    if cr0.contains(Cr0Flags::WRITE_PROTECT) {
        unsafe { Cr0::write(cr0 - Cr0Flags::WRITE_PROTECT); }
    }

    let result = unsafe { page_table.map_to(page, frame, flags, allocator) };

    if cr0.contains(Cr0Flags::WRITE_PROTECT) {
        unsafe { Cr0::write(cr0); }
    }

    match result {
        Ok(flush) => flush.flush(),
        Err(MapToError::PageAlreadyMapped(_)) => return Ok(()),
        Err(MapToError::ParentEntryHugePage) => {
            // 既存の大ページに含まれる領域はスキップ
            return Ok(());
        }
        Err(_) => return Err(KernelError::Memory(MemoryError::InvalidAddress)),
    }

    Ok(())
}

/// 仮想アドレスを物理アドレスに変換
pub fn translate_addr(addr: VirtAddr) -> Option<PhysAddr> {
    use x86_64::structures::paging::mapper::Translate;

    let page_table = PAGE_TABLE.lock();
    page_table.as_ref()?.translate_addr(addr)
}

pub use x86_64::PhysAddr;
