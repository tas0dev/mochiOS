//! ページング管理モジュール
//!
//! 仮想メモリとページテーブル管理

use crate::result::{Kernel, Memory, Result};
use crate::info;
use spin::Mutex;
use x86_64::{
    structures::paging::{
        Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
    },
    VirtAddr,
};
use crate::mem::frame;
use uefi::table::boot::MemoryType as UefiMemoryType;
use x86_64::registers::control::{Cr3, Cr3Flags};


static PAGE_TABLE: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
static PHYS_OFFSET: Mutex<Option<u64>> = Mutex::new(None);

/// ページングシステムを初期化
pub fn init(boot_info: &'static crate::BootInfo) {
    info!("Initializing paging...");

    let physical_memory_offset = boot_info.physical_memory_offset;

    // 新しいレベル4ページテーブル用のフレームを割り当て
    let l4_frame = frame::allocate_frame().expect("Failed to allocate frame for new page table");
    let l4_table_addr = l4_frame.start_address().as_u64();
    info!("New L4 table at {:#x}", l4_table_addr);

    // 新しいページテーブルを初期化
    let l4_table = unsafe { &mut *(l4_table_addr as *mut PageTable) };
    l4_table.zero();

    let mut page_table =
        unsafe { OffsetPageTable::new(l4_table, VirtAddr::new(physical_memory_offset)) };

    // フレームアロケータを取得
    let mut allocator_lock = frame::FRAME_ALLOCATOR.lock();
    let allocator = allocator_lock
        .as_mut()
        .expect("Frame allocator not initialized");

    // メモリマップに基づいて必要な領域をidentity mapする
    let memory_map = unsafe {
        core::slice::from_raw_parts(
            boot_info.memory_map_addr as *const crate::MemoryRegion,
            boot_info.memory_map_len,
        )
    };

    let mut mapped_pages = 0;

    // 現在のスタックポインタを取得して、スタックが含まれる領域を特定する
    let rsp: u64;
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) rsp);
    }
    info!("Current RSP: {:#x}", rsp);

    // マップすべき領域のタイプ
    // 基本的にOSが使用する可能性のある領域はすべてRWでマップする
    for region in memory_map {
        let is_stack = rsp >= region.start && rsp < (region.start + region.len);
        let should_map = match region.region_type {
            crate::MemoryType::Usable => true,
            crate::MemoryType::BootloaderReclaimable => true,
            crate::MemoryType::AcpiReclaimable => true,
            crate::MemoryType::AcpiNvs => true,
            crate::MemoryType::Reserved => is_stack, // スタック領域のみマップ
            crate::MemoryType::BadMemory => false,

            _ => true,
        };

        if should_map {
            crate::debug!(
                "Mapping region {:?} at {:#x}",
                region.region_type,
                region.start
            );
            let start_frame =
                PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(region.start));
            let end_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(
                region.start + region.len - 1,
            ));

            for frame in PhysFrame::range_inclusive(start_frame, end_frame) {
                let phys = frame.start_address();
                let virt = VirtAddr::new(phys.as_u64() + physical_memory_offset); // Identity map
                let page = Page::containing_address(virt);

                let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

                unsafe {
                    if let Ok(mapper) = page_table.map_to(page, frame, flags, allocator) {
                        mapper.ignore();
                        mapped_pages += 1;
                    }
                }
            }
        }
    }

    // カーネルコード領域（起動時のコード）が含まれているか確認し、マップする
    // 現在の命令ポインタ（RIP）を取得して、その周辺も確実にマップする
    let rip: u64;
    unsafe {
        core::arch::asm!("lea {}, [rip]", out(reg) rip);
    }
    info!("Current RIP: {:#x}", rip);

    // カーネルが含まれる領域を特別に検索してマップ
    for region in memory_map {
        let is_kernel = rip >= region.start && rip < (region.start + region.len);
        if is_kernel {
             crate::debug!(
                "Kernel Code in region {:?} at {:#x} - {:#x}",
                region.region_type,
                region.start,
                region.start + region.len
            );
        }
        let is_stack = rsp >= region.start && rsp < (region.start + region.len);
        if is_stack {
             crate::debug!(
                "Kernel Stack in region {:?} at {:#x} - {:#x}",
                region.region_type,
                region.start,
                region.start + region.len
            );
        }
    }

    // フレームバッファをマップ (もしメモリマップに含まれていなければ)
    let fb_start =
        PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(boot_info.framebuffer_addr));
    let fb_end = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(
        boot_info.framebuffer_addr + (boot_info.framebuffer_size as u64) - 1,
    ));
    for frame in PhysFrame::range_inclusive(fb_start, fb_end) {
        let phys = frame.start_address();
        let virt = VirtAddr::new(phys.as_u64() + physical_memory_offset);
        let page = Page::containing_address(virt);
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE; // | PageTableFlags::NO_CACHE?
        unsafe {
            if let Ok(mapper) = page_table.map_to(page, frame, flags, allocator) {
                mapper.ignore();
            }
        }
    }

    // CR3スイッチ
    drop(allocator_lock);

    crate::debug!("Switching to new page table...");
    unsafe {
        Cr3::write(l4_frame, Cr3Flags::empty());
        *PAGE_TABLE.lock() = Some(page_table);
        *PHYS_OFFSET.lock() = Some(physical_memory_offset);
    }
    crate::debug!("Switched CR3 successfully.");

    crate::debug!(
        "Paging initialized. New table active. Mapped {} pages.",
        mapped_pages
    );
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
        .ok_or(Kernel::Memory(Memory::NotMapped))?;

    let mut allocator_lock = frame::FRAME_ALLOCATOR.lock();
    let allocator = allocator_lock
        .as_mut()
        .ok_or(Kernel::Memory(
            Memory::OutOfMemory,
        ))?;

    // すでにマップされている場合はアンマップする (Identity Mappingとの競合回避)
    use x86_64::structures::paging::mapper::Translate;
    if page_table.translate_page(page).is_ok() {
        if let Ok((_, flush)) = page_table.unmap(page) {
            flush.flush();
        }
    }

    unsafe {
        page_table
            .map_to(page, frame, flags, allocator)
            .map_err(|_| Kernel::Memory(Memory::InvalidAddress))?
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
) -> Result<()> {
    use crate::mem::frame;
    use crate::result::{Kernel, Memory};
    use x86_64::structures::paging::PageTableFlags as Flags;

    let phys_off = physical_memory_offset().ok_or(Kernel::Memory(
        Memory::NotMapped,
    ))?;

    let start = vaddr & !0xfffu64;
    let end = ((vaddr + memsz + 0xfff) & !0xfffu64);

    let mut page_addr = start;
    while page_addr < end {
        let frame = frame::allocate_frame()?;
        let page = Page::containing_address(VirtAddr::new(page_addr));
        
        // 初期フラグ：PRESENT + USER_ACCESSIBLE + WRITABLE (コピーのため)
        // NO_EXECUTEビットは設定しない（デフォルトで実行可能）
        let mut flags =
            PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;
        
        crate::debug!(
            "about to map page {:#x} -> frame {:#x}, flags={:?}, writable={}",
            page_addr,
            frame.start_address().as_u64(),
            flags,
            writable
        );
        map_page(page, frame, flags)?;
        let page_start = page_addr;
        let phys_frame_addr = frame.start_address().as_u64();
        crate::debug!(
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
            let offset_into_page = (copy_start - page_start);
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
                let offset_into_page = (zero_start - page_start);
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
        // セグメントのコピーと初期化が完了したら、最終的なフラグを設定
        if !writable {
            if let Some(ref mut pt) = PAGE_TABLE.lock().as_mut() {
                // 読み取り専用: PRESENT + USER_ACCESSIBLE (WRITABLEを外す)
                let new_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
                crate::debug!("Updating page {:#x} to read-only (flags={:?})", page_addr, new_flags);
                unsafe {
                    match pt.update_flags(page, new_flags) {
                        Ok(flush) => flush.flush(),
                        Err(e) => crate::warn!("Failed to update flags for page {:#x}: {:?}", page_addr, e),
                    }
                }
            }
        } else {
            // 書き込み可能な場合も最終フラグを確認
            crate::debug!("Page {:#x} remains writable", page_addr);
        }
        page_addr += 4096;
    }

    Ok(())
}

pub use x86_64::PhysAddr;
