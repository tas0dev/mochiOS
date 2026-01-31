//! ページング管理モジュール
//!
//! 仮想メモリとページテーブル管理

use crate::error::{KernelError, MemoryError, Result};
use crate::sprintln;
use spin::Mutex;
use x86_64::{
    structures::paging::{
        Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
    },
    VirtAddr,
};

static PAGE_TABLE: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
static PHYS_OFFSET: Mutex<Option<u64>> = Mutex::new(None);

/// ページングシステムを初期化
pub fn init(physical_memory_offset: u64) {
    sprintln!("Initializing paging...");

    unsafe {
        let level_4_table = active_level_4_table(physical_memory_offset);
        let page_table = OffsetPageTable::new(level_4_table, VirtAddr::new(physical_memory_offset));
        *PAGE_TABLE.lock() = Some(page_table);
        *PHYS_OFFSET.lock() = Some(physical_memory_offset);
    }

    sprintln!("Paging initialized");
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

    unsafe {
        page_table
            .map_to(page, frame, flags, allocator)
            .map_err(|_| KernelError::Memory(MemoryError::InvalidAddress))?
            .flush();
    }

    Ok(())
}

/// 仮想アドレスを物理アドレスに変換
pub fn translate_addr(addr: VirtAddr) -> Option<PhysAddr> {
    use x86_64::structures::paging::mapper::Translate;

    let page_table = PAGE_TABLE.lock();
    page_table.as_ref()?.translate_addr(addr)
}

/// Get the physical memory offset used by the kernel (virtual = phys + offset)
pub fn physical_memory_offset() -> Option<u64> {
    *PHYS_OFFSET.lock()
}

/// Map a range of virtual addresses [vaddr, vaddr+memsz) by allocating frames and mapping them
/// Then copy file-backed bytes from `src` (which corresponds to the start of the segment) into memory.
pub fn map_and_copy_segment(
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    src: &[u8],
    writable: bool,
) -> crate::Result<()> {
    use crate::error::{KernelError, MemoryError};
    use crate::mem::frame;
    use x86_64::structures::paging::PageTableFlags as Flags;

    let phys_off = physical_memory_offset().ok_or(crate::error::KernelError::Memory(
        crate::error::MemoryError::NotMapped,
    ))?;

    let start = vaddr & !0xfffu64;
    let end = ((vaddr + memsz + 0xfff) & !0xfffu64) as u64;

    let mut page_addr = start;
    while page_addr < end {
        let frame = frame::allocate_frame()?;
        let page = Page::containing_address(VirtAddr::new(page_addr));
        let mut flags =
            PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;
        crate::debug!(
            "about to map page {:#x} -> frame {:#x}, flags={:?}, phys_off={:#x}",
            page_addr,
            frame.start_address().as_u64(),
            flags,
            phys_off
        );
        map_page(page, frame, flags)?;
        let page_start = page_addr;
        let phys_frame_addr = frame.start_address().as_u64();
        crate::info!(
            "mapped page {:#x} -> phys {:#x}",
            page_start,
            phys_frame_addr
        );
        
        if let Some(phys_check) = translate_addr(VirtAddr::new(page_start)) {
            crate::debug!(
                "translate_addr({:#x}) = phys {:#x}",
                page_start,
                phys_check.as_u64()
            );
        } else {
            crate::debug!("translate_addr({:#x}) = None", page_start);
        }
        let page_end = page_addr + 4096;
        let file_region_start = vaddr;
        let file_region_end = vaddr + filesz;
        let copy_start = core::cmp::max(page_start, file_region_start);
        let copy_end = core::cmp::min(page_end, file_region_end);
        if copy_start < copy_end {
            let src_off = (copy_start - vaddr) as usize;
            let len = (copy_end - copy_start) as usize;
            let offset_into_page = (copy_start - page_start) as u64;
            let dst_virt_addr = page_start + offset_into_page;
            let dst_virt = dst_virt_addr as *mut u8;
            crate::debug!(
                "copying {} bytes to virt {:#x} (phys {:#x})",
                len,
                dst_virt_addr,
                phys_frame_addr + offset_into_page
            );
            unsafe {
                core::ptr::copy_nonoverlapping(src.as_ptr().add(src_off), dst_virt, len);
            }
        }
        if page_start < vaddr + memsz {
            let zero_start = core::cmp::max(page_start, vaddr + filesz);
            let zero_end = core::cmp::min(page_end, vaddr + memsz);
            if zero_start < zero_end {
                let offset_into_page = (zero_start - page_start) as u64;
                let dst_virt_addr = page_start + offset_into_page;
                let dst_virt = dst_virt_addr as *mut u8;
                let len = (zero_end - zero_start) as usize;
                crate::debug!(
                    "zeroing {} bytes at virt {:#x} (phys {:#x})",
                    len,
                    dst_virt_addr,
                    phys_frame_addr + offset_into_page
                );
                unsafe { core::ptr::write_bytes(dst_virt, 0, len) };
            }
        }
        if !writable {
            if let Some(ref mut pt) = PAGE_TABLE.lock().as_mut() {
                use x86_64::structures::paging::mapper::MapToError;
                let new_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
                unsafe {
                    let _ = pt.update_flags(page, new_flags).ok();
                }
            }
        }
        page_addr += 4096;
    }

    Ok(())
}

pub use x86_64::PhysAddr;
