//! ページング管理モジュール
//!
//! 仮想メモリとページテーブル管理

use crate::info;
use crate::mem::frame;
use crate::result::{Kernel, Memory, Result};
use spin::Mutex;

use x86_64::registers::control::{Cr3, Cr3Flags};
use x86_64::{
    registers::control::{Cr0, Cr0Flags},
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame,
        Size4KiB,
    },
    VirtAddr,
};

/// アクティブなページテーブルへのグローバル参照と物理メモリオフセット
pub static PAGE_TABLE: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
/// 物理メモリオフセット（init時に設定） - 仮想アドレス = 物理アドレス + オフセット
pub static PHYS_OFFSET: Mutex<Option<u64>> = Mutex::new(None);
/// カーネルの元のL4ページテーブルの物理アドレス（init時に設定）
pub static KERNEL_L4_PHYS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// x86-64 canonical ユーザー空間上限
const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;

#[cfg(target_os = "uefi")]
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text$A")]
static __MOCHIOS_TEXT_START_MARKER: u8 = 0;
#[cfg(target_os = "uefi")]
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text$Z")]
static __MOCHIOS_TEXT_END_MARKER: u8 = 0;

#[cfg(not(target_os = "uefi"))]
unsafe extern "C" {
    static __text_start: u8;
    static __text_end: u8;
}

#[cfg(target_os = "uefi")]
fn kernel_text_range() -> (u64, u64) {
    (
        core::ptr::addr_of!(__MOCHIOS_TEXT_START_MARKER) as u64,
        core::ptr::addr_of!(__MOCHIOS_TEXT_END_MARKER) as u64,
    )
}

#[cfg(not(target_os = "uefi"))]
fn kernel_text_range() -> (u64, u64) {
    unsafe {
        (
            core::ptr::addr_of!(__text_start) as u64,
            core::ptr::addr_of!(__text_end) as u64,
        )
    }
}

fn protect_kernel_text_pages(page_table: &mut OffsetPageTable<'static>) {
    let (text_start, text_end) = kernel_text_range();
    if text_end <= text_start {
        crate::warn!(
            "Invalid .text range: start={:#x}, end={:#x}",
            text_start,
            text_end
        );
        return;
    }

    let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(text_start));
    let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(text_end - 1));

    for page in Page::<Size4KiB>::range_inclusive(start_page, end_page) {
        unsafe {
            let _ = page_table
                .update_flags(page, PageTableFlags::PRESENT)
                .map(|flush| flush.flush());
        }
    }
}

/// ページングシステムを初期化
///
/// ## Arguments
/// - `boot_info`: ブートローダーから提供される情報（メモリマップ、物理メモリオフセットなど）
pub fn init(boot_info: &'static crate::BootInfo) {
    info!("Initializing paging...");

    let physical_memory_offset = boot_info.physical_memory_offset;

    // 新しいレベル4ページテーブル用のフレームを割り当て
    let l4_frame = match frame::allocate_frame() {
        Ok(f) => f,
        Err(e) => {
            crate::warn!("Failed to allocate frame for new page table: {:?}", e);
            x86_64::instructions::interrupts::disable();
            loop {
                x86_64::instructions::hlt();
            }
        }
    };
    let l4_table_addr = l4_frame.start_address().as_u64();
    info!("New L4 table at {:#x}", l4_table_addr);

    // 新しいページテーブルを初期化
    let l4_table = unsafe { &mut *(l4_table_addr as *mut PageTable) };
    l4_table.zero();

    let mut page_table =
        unsafe { OffsetPageTable::new(l4_table, VirtAddr::new(physical_memory_offset)) };

    // フレームアロケータを取得
    let mut allocator_lock = frame::FRAME_ALLOCATOR.lock();
    let allocator = match allocator_lock.as_mut() {
        Some(a) => a,
        None => {
            crate::warn!("Frame allocator not initialized");
            loop {
                x86_64::instructions::hlt();
            }
        }
    };

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

    // HIGH-06 対応: カーネル .text は読み取り専用に戻す
    protect_kernel_text_pages(&mut page_table);

    // CR3スイッチ
    drop(allocator_lock);

    crate::debug!("Switching to new page table...");
    unsafe {
        Cr3::write(l4_frame, Cr3Flags::empty());
        *PAGE_TABLE.lock() = Some(page_table);
        *PHYS_OFFSET.lock() = Some(physical_memory_offset);
        // カーネルの元のページテーブルアドレスを保存
        KERNEL_L4_PHYS.store(l4_table_addr, core::sync::atomic::Ordering::Relaxed);
    }
    // フレームアロケータに HHDM オフセットを伝えてフリーリストを有効化
    super::frame::set_phys_offset(physical_memory_offset);
    crate::debug!("Switched CR3 successfully.");

    crate::debug!(
        "Paging initialized. New table active. Mapped {} pages.",
        mapped_pages
    );
}

/// アクティブなレベル4ページテーブルへの参照を取得
///
/// ## Arguments
/// - `physical_memory_offset`: カーネルが使用する物理メモリオフセット（仮想アドレス = 物理アドレス + オフセット）
///
/// ## Returns
/// アクティブなレベル4ページテーブルへのミュータブル参照
unsafe fn active_level_4_table(physical_memory_offset: u64) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();
    let phys = level_4_table_frame.start_address();
    let virt = VirtAddr::new(phys.as_u64() + physical_memory_offset);
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr
}

/// ページをマップ
///
/// ## Arguments
/// - `page`: マップする仮想ページ
/// - `frame`: マップ先の物理フレーム
/// - `flags`: ページテーブルエントリのフラグ（例: PRESENT, WRITABLE, USER_ACCESSIBLEなど）
///
/// ## Returns
/// 成功した場合は `Ok(())`、失敗した場合はエラーを返す
pub fn map_page(page: Page, frame: PhysFrame, flags: PageTableFlags) -> Result<()> {
    let mut page_table_lock = PAGE_TABLE.lock();
    let page_table = page_table_lock
        .as_mut()
        .ok_or(Kernel::Memory(Memory::NotMapped))?;

    let mut allocator_lock = frame::FRAME_ALLOCATOR.lock();
    let allocator = allocator_lock
        .as_mut()
        .ok_or(Kernel::Memory(Memory::OutOfMemory))?;

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
///
/// ## Arguments
/// - `addr`: 変換する仮想アドレス
///
/// ## Returns
/// 変換された物理アドレス、または変換できない場合は `None`
pub fn translate_addr(addr: VirtAddr) -> Option<PhysAddr> {
    use x86_64::structures::paging::mapper::Translate;

    let page_table = PAGE_TABLE.lock();
    page_table.as_ref()?.translate_addr(addr)
}

/// 指定したページテーブル上で仮想アドレスを物理アドレスへ変換する
pub fn translate_addr_in_table(
    table_phys: u64,
    addr: VirtAddr,
) -> Option<(PhysAddr, PageTableFlags)> {
    let phys_off = physical_memory_offset()?;
    if (table_phys & 0xfff) != 0 {
        return None;
    }

    let l4_vaddr = table_phys.checked_add(phys_off)?;
    let l4 = unsafe { &*(l4_vaddr as *const PageTable) };
    let l4i = addr.p4_index();
    let l4e = &l4[l4i];
    if l4e.is_unused() || !l4e.flags().contains(PageTableFlags::PRESENT) {
        return None;
    }

    let l3_vaddr = l4e.addr().as_u64().checked_add(phys_off)?;
    let l3 = unsafe { &*(l3_vaddr as *const PageTable) };
    let l3i = addr.p3_index();
    let l3e = &l3[l3i];
    let l3f = l3e.flags();
    if l3e.is_unused() || !l3f.contains(PageTableFlags::PRESENT) {
        return None;
    }
    if l3f.contains(PageTableFlags::HUGE_PAGE) {
        let page_off = addr.as_u64() & ((1u64 << 30) - 1);
        return Some((
            PhysAddr::new(l3e.addr().as_u64().checked_add(page_off)?),
            l3f,
        ));
    }

    let l2_vaddr = l3e.addr().as_u64().checked_add(phys_off)?;
    let l2 = unsafe { &*(l2_vaddr as *const PageTable) };
    let l2i = addr.p2_index();
    let l2e = &l2[l2i];
    let l2f = l2e.flags();
    if l2e.is_unused() || !l2f.contains(PageTableFlags::PRESENT) {
        return None;
    }
    if l2f.contains(PageTableFlags::HUGE_PAGE) {
        let page_off = addr.as_u64() & ((1u64 << 21) - 1);
        return Some((
            PhysAddr::new(l2e.addr().as_u64().checked_add(page_off)?),
            l2f,
        ));
    }

    let l1_vaddr = l2e.addr().as_u64().checked_add(phys_off)?;
    let l1 = unsafe { &*(l1_vaddr as *const PageTable) };
    let l1i = addr.p1_index();
    let l1e = &l1[l1i];
    let l1f = l1e.flags();
    if l1e.is_unused() || !l1f.contains(PageTableFlags::PRESENT) {
        return None;
    }

    let page_off = addr.as_u64() & 0xfff;
    Some((
        PhysAddr::new(l1e.addr().as_u64().checked_add(page_off)?),
        l1f,
    ))
}

/// 指定したページテーブル上の仮想アドレスからu64値を読み出す
pub fn read_u64_in_table(table_phys: u64, vaddr: u64) -> Option<u64> {
    let phys_off = physical_memory_offset()?;
    let (phys, _) = translate_addr_in_table(table_phys, VirtAddr::new(vaddr))?;
    let ptr = phys.as_u64().checked_add(phys_off)? as *const u64;
    Some(unsafe { core::ptr::read_unaligned(ptr) })
}

/// 物理メモリオフセットを取得
///
/// ## Returns
/// カーネルが使用する物理メモリオフセット（仮想アドレス = 物理アドレス + オフセット）
pub fn physical_memory_offset() -> Option<u64> {
    *PHYS_OFFSET.lock()
}

fn user_page_flags_in_table(table_phys: u64, page_addr: u64) -> Option<PageTableFlags> {
    let phys_off = physical_memory_offset()?;
    if (table_phys & 0xfff) != 0 {
        return None;
    }

    let l4_vaddr = table_phys.checked_add(phys_off)?;
    let l4 = unsafe { &*(l4_vaddr as *const PageTable) };
    let l4i = ((page_addr >> 39) & 0x1ff) as usize;
    if l4i >= 256 {
        return None;
    }
    let l4e = &l4[l4i];
    let l4f = l4e.flags();
    if l4e.is_unused()
        || !l4f.contains(PageTableFlags::PRESENT)
        || !l4f.contains(PageTableFlags::USER_ACCESSIBLE)
    {
        return None;
    }

    let l3_vaddr = l4e.addr().as_u64().checked_add(phys_off)?;
    let l3 = unsafe { &*(l3_vaddr as *const PageTable) };
    let l3i = ((page_addr >> 30) & 0x1ff) as usize;
    let l3e = &l3[l3i];
    let l3f = l3e.flags();
    if l3e.is_unused()
        || !l3f.contains(PageTableFlags::PRESENT)
        || !l3f.contains(PageTableFlags::USER_ACCESSIBLE)
    {
        return None;
    }
    if l3f.contains(PageTableFlags::HUGE_PAGE) {
        return Some(l3f);
    }

    let l2_vaddr = l3e.addr().as_u64().checked_add(phys_off)?;
    let l2 = unsafe { &*(l2_vaddr as *const PageTable) };
    let l2i = ((page_addr >> 21) & 0x1ff) as usize;
    let l2e = &l2[l2i];
    let l2f = l2e.flags();
    if l2e.is_unused()
        || !l2f.contains(PageTableFlags::PRESENT)
        || !l2f.contains(PageTableFlags::USER_ACCESSIBLE)
    {
        return None;
    }
    if l2f.contains(PageTableFlags::HUGE_PAGE) {
        return Some(l2f);
    }

    let l1_vaddr = l2e.addr().as_u64().checked_add(phys_off)?;
    let l1 = unsafe { &*(l1_vaddr as *const PageTable) };
    let l1i = ((page_addr >> 12) & 0x1ff) as usize;
    let l1e = &l1[l1i];
    let l1f = l1e.flags();
    if l1e.is_unused()
        || !l1f.contains(PageTableFlags::PRESENT)
        || !l1f.contains(PageTableFlags::USER_ACCESSIBLE)
    {
        return None;
    }
    Some(l1f)
}

fn page_is_user_mapped_in_table(table_phys: u64, page_addr: u64) -> bool {
    user_page_flags_in_table(table_phys, page_addr).is_some_and(|flags| {
        flags.contains(PageTableFlags::PRESENT) && flags.contains(PageTableFlags::USER_ACCESSIBLE)
    })
}

/// 指定したページテーブルでユーザー範囲がすべて有効にマップされているか確認する
pub fn is_user_range_mapped_in_table(table_phys: u64, addr: u64, len: u64) -> bool {
    if addr > USER_SPACE_END {
        return false;
    }
    if len == 0 {
        return true;
    }

    let end_inclusive = match addr.checked_add(len.saturating_sub(1)) {
        Some(v) if v <= USER_SPACE_END => v,
        _ => return false,
    };

    let mut page_addr = addr & !0xfffu64;
    let end_page = end_inclusive & !0xfffu64;
    loop {
        if !page_is_user_mapped_in_table(table_phys, page_addr) {
            return false;
        }
        if page_addr == end_page {
            return true;
        }
        page_addr = match page_addr.checked_add(4096) {
            Some(v) => v,
            None => return false,
        };
    }
}

/// 指定した仮想アドレス範囲にセグメントをマップしてコピーする
///
/// データはカーネルの恒等マッピング（phys = virt）経由で物理フレームに直接書き込む。
///
/// ## Arguments
/// - `vaddr`: セグメントの開始仮想アドレス
/// - `filesz`: セグメントのファイルサイズ（ELFヘッダのp_filesz）
/// - `memsz`: セグメントのメモリサイズ（ELFヘッダのp_memsz）
/// - `src`: セグメントのデータが格納されたバッファ
/// - `writable`: セグメントをRWXのどれでマップするか
/// - `executable`: セグメントをRWXのどれでマップするか
///
/// ## Returns
/// 成功した場合は `Ok(())`、失敗した場合はエラー
pub fn map_and_copy_segment(
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    src: &[u8],
    writable: bool,
    executable: bool,
) -> Result<()> {
    use crate::mem::frame;
    use crate::result::{Kernel, Memory};

    let phys_off = physical_memory_offset().ok_or(Kernel::Memory(Memory::NotMapped))?;
    if memsz == 0 {
        return if filesz == 0 {
            Ok(())
        } else {
            Err(Kernel::InvalidParam)
        };
    }
    if memsz < filesz || (filesz as usize) > src.len() {
        return Err(Kernel::InvalidParam);
    }
    let file_end = vaddr
        .checked_add(filesz)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;
    let mem_end = vaddr
        .checked_add(memsz)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;
    let start = vaddr & !0xfffu64;
    let end = mem_end
        .checked_add(0xfff)
        .map(|v| v & !0xfffu64)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;

    let mut page_addr = start;
    while page_addr < end {
        let page = Page::containing_address(VirtAddr::new(page_addr));
        let phys_frame_addr;

        // Check if page is already mapped
        let is_mapped = translate_addr(VirtAddr::new(page_addr)).is_some();

        if is_mapped {
            // Already mapped. Ensure it is writable for loading.
            phys_frame_addr = translate_addr(VirtAddr::new(page_addr))
                .ok_or(Kernel::Memory(Memory::InvalidAddress))?
                .as_u64();

            // Temporarily map as writable for loading, but preserve execute permission
            // to avoid conflicts with final flags
            let flags = PageTableFlags::PRESENT
                | PageTableFlags::USER_ACCESSIBLE
                | PageTableFlags::WRITABLE;
            // Don't set NO_EXECUTE during loading - we'll set it in the final flag update if needed

            if let Some(ref mut pt) = PAGE_TABLE.lock().as_mut() {
                unsafe {
                    // Update flags ignoring error (e.g. if already same)
                    let _ = pt.update_flags(page, flags).map(|f| f.flush());
                }
            }

            crate::debug!(
                "reusing mapped page {:#x} -> phys {:#x}",
                page_addr,
                phys_frame_addr
            );
        } else {
            // Not mapped, allocate new frame
            let frame = frame::allocate_frame()?;

            // Setup flags: PRESENT + USER + WRITABLE
            let flags = PageTableFlags::PRESENT
                | PageTableFlags::USER_ACCESSIBLE
                | PageTableFlags::WRITABLE;

            crate::debug!(
                "about to map page {:#x} -> frame {:#x}, flags={:?}, writable={}",
                page_addr,
                frame.start_address().as_u64(),
                flags,
                writable
            );
            map_page(page, frame, flags)?;
            phys_frame_addr = frame.start_address().as_u64();
            crate::debug!(
                "mapped page {:#x} -> phys {:#x}",
                page_addr,
                phys_frame_addr
            );
        }

        let page_start = page_addr;
        let page_end = page_addr + 4096;
        let copy_start = core::cmp::max(page_start, vaddr);
        let copy_end = core::cmp::min(page_end, file_end);
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
        if page_start < mem_end {
            let zero_start = core::cmp::max(page_start, file_end);
            let zero_end = core::cmp::min(page_end, mem_end);
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
        if let Some(ref mut pt) = PAGE_TABLE.lock().as_mut() {
            let mut new_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;

            if writable {
                new_flags |= PageTableFlags::WRITABLE;
            }

            // NX (No-Execute) bit: set it for non-executable pages
            if !executable {
                new_flags |= PageTableFlags::NO_EXECUTE;
            }

            crate::debug!(
                "Updating page {:#x} flags: writable={}, executable={}, flags={:?}",
                page_addr,
                writable,
                executable,
                new_flags
            );

            unsafe {
                // まず既存のマッピングを解除
                if let Ok((_, flush)) = pt.unmap(page) {
                    flush.flush();
                }

                // 同じ物理フレームに新しいフラグで再マップ
                let phys_frame = PhysFrame::containing_address(PhysAddr::new(phys_frame_addr));
                {
                    let mut alloc_lock = frame::FRAME_ALLOCATOR.lock();
                    let alloc_ref = alloc_lock
                        .as_mut()
                        .ok_or(Kernel::Memory(Memory::OutOfMemory))?;
                    match pt.map_to(page, phys_frame, new_flags, alloc_ref) {
                        Ok(flush) => flush.flush(),
                        Err(e) => crate::warn!("Failed to remap page {:#x}: {:?}", page_addr, e),
                    }
                }
            }
        }
        page_addr += 4096;
    }

    Ok(())
}

pub use x86_64::PhysAddr;

fn clone_kernel_l1_table_without_user_entries(src_l1_phys: u64, phys_off: u64) -> Result<u64> {
    let src_l1 = unsafe { &*((src_l1_phys + phys_off) as *const PageTable) };
    let new_l1_frame = frame::allocate_frame()?;
    let new_l1_phys = new_l1_frame.start_address().as_u64();
    let new_l1 = unsafe { &mut *((new_l1_phys + phys_off) as *mut PageTable) };
    new_l1.zero();

    for i in 0..512 {
        let entry = src_l1[i].clone();
        let flags = entry.flags();
        if entry.is_unused() || !flags.contains(PageTableFlags::PRESENT) {
            continue;
        }
        if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            continue;
        }
        new_l1[i] = entry;
    }

    Ok(new_l1_phys)
}

fn clone_kernel_l2_table_without_user_entries(src_l2_phys: u64, phys_off: u64) -> Result<u64> {
    let src_l2 = unsafe { &*((src_l2_phys + phys_off) as *const PageTable) };
    let new_l2_frame = frame::allocate_frame()?;
    let new_l2_phys = new_l2_frame.start_address().as_u64();
    let new_l2 = unsafe { &mut *((new_l2_phys + phys_off) as *mut PageTable) };
    new_l2.zero();

    for i in 0..512 {
        let entry = src_l2[i].clone();
        let flags = entry.flags();
        if entry.is_unused() || !flags.contains(PageTableFlags::PRESENT) {
            continue;
        }

        if flags.contains(PageTableFlags::HUGE_PAGE) {
            if !flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                new_l2[i] = entry;
            }
            continue;
        }

        let new_l1_phys =
            clone_kernel_l1_table_without_user_entries(entry.addr().as_u64(), phys_off)?;
        new_l2[i].set_addr(PhysAddr::new(new_l1_phys), flags);
    }

    Ok(new_l2_phys)
}

/// ユーザープロセス用の新しいL4ページテーブルを作成する
///
/// カーネルのページテーブル階層を部分的にコピーして、カーネルメモリには
/// アクセス可能だがユーザー空間は空（プロセス固有）の新しいページテーブルを作成する。
///
/// アドレス空間レイアウト（phys_off=0, identity mapping）:
///   - 0x200000 (L4[0]→L3[0]→L2[1]) : ユーザーコード（プロセス固有）
///   - 0x179d... (L4[0]→L3[0]→L2[188-189]): カーネルスタック（共有）
///   - 0x139... (L4[0]→L3[0]→L2[458]): カーネルコード（共有）
///   - 0x7FFF_FFF0_0000 (L4[255]): ユーザースタック（プロセス固有）
///
/// ## Returns
/// 新しいL4テーブルの物理アドレス
pub fn create_user_page_table() -> Result<u64> {
    let phys_off = physical_memory_offset().ok_or(Kernel::Memory(Memory::NotMapped))?;

    // カーネルの「元の」L4テーブルを使用する（syscall中はCR3がユーザープロセスのテーブルなため）
    let kernel_l4_phys = KERNEL_L4_PHYS.load(core::sync::atomic::Ordering::Relaxed);
    if kernel_l4_phys == 0 {
        return Err(Kernel::Memory(Memory::NotMapped));
    }
    let kernel_l4 = unsafe { &*((kernel_l4_phys + phys_off) as *const PageTable) };

    // 新しいL4フレームを確保してゼロ初期化
    let new_l4_frame = frame::allocate_frame()?;
    let new_l4_phys = new_l4_frame.start_address().as_u64();
    let new_l4 = unsafe { &mut *((new_l4_phys + phys_off) as *mut PageTable) };
    new_l4.zero();

    // KPTI強化: L4[0]（低位512GiB）のみ最小限コピーする。
    // これにより上位L4エントリを通じた広域カーネルマッピングをユーザーテーブルから除外する。
    // 実際のユーザー領域は exec/mmap 時に個別マップされる。
    if !kernel_l4[0].is_unused() {
        let kernel_l3_phys = kernel_l4[0].addr().as_u64();
        let kernel_l3 = unsafe { &*((kernel_l3_phys + phys_off) as *const PageTable) };

        let new_l3_frame = frame::allocate_frame()?;
        let new_l3_phys = new_l3_frame.start_address().as_u64();
        let new_l3 = unsafe { &mut *((new_l3_phys + phys_off) as *mut PageTable) };
        new_l3.zero();

        // L3[0]: 最初の1GB（カーネルコード・スタックとユーザーコードが混在）
        if !kernel_l3[0].is_unused() {
            let kernel_l2_phys = kernel_l3[0].addr().as_u64();
            let new_l2_phys = clone_kernel_l2_table_without_user_entries(kernel_l2_phys, phys_off)?;

            new_l3[0].set_addr(PhysAddr::new(new_l2_phys), kernel_l3[0].flags());
        }

        new_l4[0].set_addr(PhysAddr::new(new_l3_phys), kernel_l4[0].flags());
    }

    // カーネルヒープ (0x4444_4444_0000, L4[136]) をユーザーページテーブルと共有する。
    // with_user_memory_access がユーザーCR3に切り替えた際に
    // カーネルヒープ上のデータ（FileHandle, Box<[u8]> など）へアクセスできるようにする。
    // ヒープは init_memory 時に全ページがマップ済みのため、
    // L4エントリ（L3テーブルへのポインタ）を共有するだけで十分。
    const KERNEL_HEAP_L4_IDX: usize = 136; // 0x4444_4444_0000 >> 39 & 0x1ff
    if !kernel_l4[KERNEL_HEAP_L4_IDX].is_unused() {
        new_l4[KERNEL_HEAP_L4_IDX] = kernel_l4[KERNEL_HEAP_L4_IDX].clone();
    }

    Ok(new_l4_phys)
}

/// 既存のユーザーページテーブルをフルコピーして新しいページテーブルを返す
///
/// - カーネル共有マッピングは `create_user_page_table()` により初期化
/// - USER_ACCESSIBLE な4KiBページを新規フレームへコピー
pub fn clone_user_page_table(src_table_phys: u64) -> Result<u64> {
    use x86_64::structures::paging::PageTableFlags as Flags;

    struct DstTableGuard(Option<u64>);
    impl DstTableGuard {
        fn disarm(&mut self) {
            self.0 = None;
        }
    }
    impl Drop for DstTableGuard {
        fn drop(&mut self) {
            if let Some(phys) = self.0 {
                let _ = destroy_user_page_table(phys);
            }
        }
    }

    let phys_off = physical_memory_offset().ok_or(Kernel::Memory(Memory::NotMapped))?;
    let dst_table_phys = create_user_page_table()?;
    let mut dst_guard = DstTableGuard(Some(dst_table_phys));

    let src_l4 = unsafe { &*((src_table_phys + phys_off) as *const PageTable) };
    let dst_l4 = unsafe { &mut *((dst_table_phys + phys_off) as *mut PageTable) };
    let mut dst_pt = unsafe { OffsetPageTable::new(dst_l4, VirtAddr::new(phys_off)) };

    for l4i in 0usize..256 {
        let l4e = &src_l4[l4i];
        if l4e.is_unused() || !l4e.flags().contains(Flags::PRESENT) {
            continue;
        }
        let src_l3 = unsafe { &*((l4e.addr().as_u64() + phys_off) as *const PageTable) };
        for l3i in 0usize..512 {
            let l3e = &src_l3[l3i];
            if l3e.is_unused() || !l3e.flags().contains(Flags::PRESENT) {
                continue;
            }
            if l3e.flags().contains(Flags::HUGE_PAGE) {
                continue;
            }
            let src_l2 = unsafe { &*((l3e.addr().as_u64() + phys_off) as *const PageTable) };
            for l2i in 0usize..512 {
                let l2e = &src_l2[l2i];
                if l2e.is_unused() || !l2e.flags().contains(Flags::PRESENT) {
                    continue;
                }
                if l2e.flags().contains(Flags::HUGE_PAGE) {
                    continue;
                }
                let src_l1 = unsafe { &*((l2e.addr().as_u64() + phys_off) as *const PageTable) };
                for l1i in 0usize..512 {
                    let pte = &src_l1[l1i];
                    if pte.is_unused() {
                        continue;
                    }
                    let src_flags = pte.flags();
                    if !src_flags.contains(Flags::PRESENT)
                        || !src_flags.contains(Flags::USER_ACCESSIBLE)
                    {
                        continue;
                    }

                    let vaddr = ((l4i as u64) << 39)
                        | ((l3i as u64) << 30)
                        | ((l2i as u64) << 21)
                        | ((l1i as u64) << 12);
                    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(vaddr));

                    let new_frame = frame::allocate_frame()?;
                    let mut dst_flags = Flags::PRESENT | Flags::USER_ACCESSIBLE;
                    if src_flags.contains(Flags::WRITABLE) {
                        dst_flags |= Flags::WRITABLE;
                    }
                    if src_flags.contains(Flags::NO_EXECUTE) {
                        dst_flags |= Flags::NO_EXECUTE;
                    }

                    unsafe {
                        let mut alloc_lock = frame::FRAME_ALLOCATOR.lock();
                        let alloc_ref = alloc_lock
                            .as_mut()
                            .ok_or(Kernel::Memory(Memory::OutOfMemory))?;
                        match dst_pt.map_to(page, new_frame, dst_flags, alloc_ref) {
                            Ok(flush) => flush.ignore(),
                            Err(
                                x86_64::structures::paging::mapper::MapToError::PageAlreadyMapped(
                                    _,
                                ),
                            ) => {
                                let (old_frame, flush) = dst_pt
                                    .unmap(page)
                                    .map_err(|_| Kernel::Memory(Memory::InvalidAddress))?;
                                flush.ignore();
                                let _ = frame::deallocate_frame(old_frame);
                                let mut alloc_lock2 = frame::FRAME_ALLOCATOR.lock();
                                let alloc_ref2 = alloc_lock2
                                    .as_mut()
                                    .ok_or(Kernel::Memory(Memory::OutOfMemory))?;
                                dst_pt
                                    .map_to(page, new_frame, dst_flags, alloc_ref2)
                                    .map_err(|_| Kernel::Memory(Memory::InvalidAddress))?
                                    .ignore();
                            }
                            Err(_) => return Err(Kernel::Memory(Memory::InvalidAddress)),
                        }
                    }

                    let src_ptr = (pte.addr().as_u64() + phys_off) as *const u8;
                    let dst_ptr = (new_frame.start_address().as_u64() + phys_off) as *mut u8;
                    unsafe {
                        core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, 4096);
                    }
                }
            }
        }
    }

    dst_guard.disarm();
    Ok(dst_table_phys)
}

/// 指定したページテーブル（物理アドレス）にセグメントをマップしてコピーする
///
/// データはカーネルの恒等マッピング（phys = virt）経由で物理フレームに直接書き込む。
/// フラッシュはカレントCR3に対しては不要なため `.ignore()` を使う。
///
/// ## Arguments
/// - `table_phys`: マップ先のページテーブルの物理アドレス
/// - `vaddr`: セグメントの開始仮想アドレス
/// - `filesz`: セグメントのファイルサイズ（ELFヘッダのp_filesz）
/// - `memsz`: セグメントのメモリサイズ（ELFヘッダのp_memsz）
/// - `src`: セグメントのデータが格納されたバッファ
/// - `writable`: セグメントをRWXのどれでマップするか
/// - `executable`: セグメントをRWXのどれでマップするか
///
/// ## Returns
/// 成功した場合は `Ok(())`、失敗した場合はエラー
pub fn map_and_copy_segment_to(
    table_phys: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    src: &[u8],
    writable: bool,
    executable: bool,
) -> Result<()> {
    use crate::result::{Kernel, Memory};
    use x86_64::structures::paging::PageTableFlags as Flags;

    let phys_off = physical_memory_offset().ok_or(Kernel::Memory(Memory::NotMapped))?;
    if memsz == 0 {
        return if filesz == 0 {
            Ok(())
        } else {
            Err(Kernel::InvalidParam)
        };
    }
    if memsz < filesz || (filesz as usize) > src.len() {
        return Err(Kernel::InvalidParam);
    }
    let file_end = vaddr
        .checked_add(filesz)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;
    let mem_end = vaddr
        .checked_add(memsz)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;
    let l4 = unsafe { &mut *((table_phys + phys_off) as *mut PageTable) };
    let mut pt = unsafe { OffsetPageTable::new(l4, VirtAddr::new(phys_off)) };

    let mut final_flags = Flags::PRESENT | Flags::USER_ACCESSIBLE;
    if writable {
        final_flags |= Flags::WRITABLE;
    }
    if !executable {
        final_flags |= Flags::NO_EXECUTE;
    }

    let start = vaddr & !0xfffu64;
    let end = mem_end
        .checked_add(0xfff)
        .map(|v| v & !0xfffu64)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;

    let mut page_addr = start;
    while page_addr < end {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_addr));

        let frame = {
            let mut alloc = frame::FRAME_ALLOCATOR.lock();
            alloc
                .as_mut()
                .ok_or(Kernel::Memory(Memory::OutOfMemory))?
                .allocate_frame()
                .ok_or(Kernel::Memory(Memory::OutOfMemory))?
        };
        let mut phys_frame_addr = frame.start_address().as_u64();

        // フレームを先にゼロ初期化（BSS領域のため）
        unsafe {
            core::ptr::write_bytes((phys_frame_addr + phys_off) as *mut u8, 0, 4096);
        }

        // マップ（既にマップ済みの場合は既存マッピングを確認して処理）
        let map_result = unsafe {
            let mut alloc_lock = frame::FRAME_ALLOCATOR.lock();
            let alloc_ref = alloc_lock
                .as_mut()
                .ok_or(Kernel::Memory(Memory::OutOfMemory))?;
            pt.map_to(page, frame, final_flags, alloc_ref)
        };
        match map_result {
            Ok(flush) => {
                flush.ignore();
            }
            Err(x86_64::structures::paging::mapper::MapToError::PageAlreadyMapped(_)) => {
                use x86_64::structures::paging::mapper::TranslateResult;
                use x86_64::structures::paging::Translate;
                unsafe {
                    match pt.translate(VirtAddr::new(page_addr)) {
                        TranslateResult::Mapped {
                            flags: existing_flags,
                            frame: existing_mapped_frame,
                            ..
                        } if existing_flags.contains(Flags::USER_ACCESSIBLE) => {
                            // 別のELFセグメントが同じページをマップ済み：パーミッションをマージする。
                            // 既存マッピングが実行可能なら新セグメントのNXビットを消してEXECを保持。
                            let merged = if !existing_flags.contains(Flags::NO_EXECUTE) {
                                final_flags & !Flags::NO_EXECUTE
                            } else {
                                final_flags
                            };
                            pt.update_flags(page, merged)
                                .map_err(|_| Kernel::Memory(Memory::InvalidAddress))?
                                .ignore();
                            // 新たに確保したフレームは不要なので解放
                            frame::deallocate_frame(frame);
                            // データコピー先を既存フレームに切り替える
                            phys_frame_addr = existing_mapped_frame.start_address().as_u64();
                        }
                        _ => {
                            // カーネルのアイデンティティマップが残っている場合：アンマップして再マップ
                            let (old_frame, flush) = pt
                                .unmap(page)
                                .map_err(|_| Kernel::Memory(Memory::InvalidAddress))?;
                            flush.ignore();
                            // 既存の supervisor-only マッピングはカーネル共有フレームなので解放しない。
                            let _ = old_frame;
                            let mut alloc_lock2 = frame::FRAME_ALLOCATOR.lock();
                            let alloc_ref2 = alloc_lock2
                                .as_mut()
                                .ok_or(Kernel::Memory(Memory::OutOfMemory))?;
                            pt.map_to(page, frame, final_flags, alloc_ref2)
                                .map_err(|_| Kernel::Memory(Memory::InvalidAddress))?
                                .ignore();
                        }
                    }
                }
            }
            Err(_) => return Err(Kernel::Memory(Memory::InvalidAddress)),
        }

        // ELFデータを物理フレームに直接書き込む（phys_off=0のためphys=virtで直接アクセス可能）
        let page_start = page_addr;
        let page_end = page_addr + 4096;
        let copy_start = core::cmp::max(page_start, vaddr);
        let copy_end = core::cmp::min(page_end, file_end);
        if copy_start < copy_end {
            let src_off = (copy_start - vaddr) as usize;
            let dst_off = (copy_start - page_start) as usize;
            let len = (copy_end - copy_start) as usize;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src.as_ptr().add(src_off),
                    (phys_frame_addr + phys_off + dst_off as u64) as *mut u8,
                    len,
                );
            }
        }

        page_addr += 4096;
    }
    Ok(())
}

/// 指定したページテーブルでユーザー範囲をアンマップし、対応フレームを解放する
pub fn unmap_range_in_table(table_phys: u64, addr: u64, length: u64) -> Result<()> {
    if length == 0 {
        return Ok(());
    }
    let phys_off = physical_memory_offset().ok_or(Kernel::Memory(Memory::NotMapped))?;
    let end_raw = addr
        .checked_add(length)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;
    let start = addr & !0xfffu64;
    let end = end_raw
        .checked_add(0xfff)
        .map(|v| v & !0xfffu64)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;

    let l4 = unsafe { &mut *((table_phys + phys_off) as *mut PageTable) };
    let mut pt = unsafe { OffsetPageTable::new(l4, VirtAddr::new(phys_off)) };

    let mut page_addr = start;
    while page_addr < end {
        if !page_is_user_mapped_in_table(table_phys, page_addr) {
            page_addr += 4096;
            continue;
        }
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_addr));
        if let Ok((frame, flush)) = pt.unmap(page) {
            flush.ignore();
            let _ = frame::deallocate_frame(frame);
        }
        page_addr += 4096;
    }
    Ok(())
}

/// アンマップするが、フレームの解放は行わない（フレーム所有権を移すときに使用）
pub fn unmap_range_in_table_preserve_frames(table_phys: u64, addr: u64, length: u64) -> Result<()> {
    if length == 0 {
        return Ok(());
    }
    let phys_off = physical_memory_offset().ok_or(Kernel::Memory(Memory::NotMapped))?;
    let end_raw = addr
        .checked_add(length)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;
    let start = addr & !0xfffu64;
    let end = end_raw
        .checked_add(0xfff)
        .map(|v| v & !0xfffu64)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;

    let l4 = unsafe { &mut *((table_phys + phys_off) as *mut PageTable) };
    let mut pt = unsafe { OffsetPageTable::new(l4, VirtAddr::new(phys_off)) };

    let mut page_addr = start;
    while page_addr < end {
        if !page_is_user_mapped_in_table(table_phys, page_addr) {
            page_addr += 4096;
            continue;
        }
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_addr));
        if let Ok((_, flush)) = pt.unmap(page) {
            // do not deallocate the physical frame; ownership transferred
            flush.ignore();
        }
        page_addr += 4096;
    }
    Ok(())
}

fn deallocate_4k_frame_by_phys(frame_phys: u64) {
    let frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(frame_phys));
    let _ = frame::deallocate_frame(frame);
}

fn destroy_user_l1_table(l1_phys: u64, phys_off: u64) {
    let l1 = unsafe { &mut *((l1_phys + phys_off) as *mut PageTable) };
    for i in 0..512 {
        let entry = l1[i].clone();
        let flags = entry.flags();
        if entry.is_unused()
            || !flags.contains(PageTableFlags::PRESENT)
            || !flags.contains(PageTableFlags::USER_ACCESSIBLE)
        {
            continue;
        }
        deallocate_4k_frame_by_phys(entry.addr().as_u64());
        l1[i].set_unused();
    }
    deallocate_4k_frame_by_phys(l1_phys);
}

fn destroy_user_l2_table(l2_phys: u64, phys_off: u64) {
    let l2 = unsafe { &mut *((l2_phys + phys_off) as *mut PageTable) };
    for i in 0..512 {
        let entry = l2[i].clone();
        let flags = entry.flags();
        if entry.is_unused()
            || !flags.contains(PageTableFlags::PRESENT)
            || !flags.contains(PageTableFlags::USER_ACCESSIBLE)
        {
            continue;
        }
        if flags.contains(PageTableFlags::HUGE_PAGE) {
            l2[i].set_unused();
            continue;
        }
        destroy_user_l1_table(entry.addr().as_u64(), phys_off);
        l2[i].set_unused();
    }
    deallocate_4k_frame_by_phys(l2_phys);
}

fn destroy_user_l3_table(l3_phys: u64, phys_off: u64) {
    let l3 = unsafe { &mut *((l3_phys + phys_off) as *mut PageTable) };
    for i in 0..512 {
        let entry = l3[i].clone();
        let flags = entry.flags();
        if entry.is_unused()
            || !flags.contains(PageTableFlags::PRESENT)
            || !flags.contains(PageTableFlags::USER_ACCESSIBLE)
        {
            continue;
        }
        if flags.contains(PageTableFlags::HUGE_PAGE) {
            l3[i].set_unused();
            continue;
        }
        destroy_user_l2_table(entry.addr().as_u64(), phys_off);
        l3[i].set_unused();
    }
    deallocate_4k_frame_by_phys(l3_phys);
}

/// 失敗したfork/exec経路のロールバック用に、ユーザーページテーブルを破棄する
pub fn destroy_user_page_table(table_phys: u64) -> Result<()> {
    if table_phys == 0 || (table_phys & 0xfff) != 0 {
        return Err(Kernel::InvalidParam);
    }
    let phys_off = physical_memory_offset().ok_or(Kernel::Memory(Memory::NotMapped))?;
    let l4 = unsafe { &mut *((table_phys + phys_off) as *mut PageTable) };

    for i in 0usize..256 {
        let entry = l4[i].clone();
        let flags = entry.flags();
        if entry.is_unused()
            || !flags.contains(PageTableFlags::PRESENT)
            || !flags.contains(PageTableFlags::USER_ACCESSIBLE)
        {
            continue;
        }
        if flags.contains(PageTableFlags::HUGE_PAGE) {
            l4[i].set_unused();
            continue;
        }
        destroy_user_l3_table(entry.addr().as_u64(), phys_off);
        l4[i].set_unused();
    }

    deallocate_4k_frame_by_phys(table_phys);
    Ok(())
}

/// 物理アドレス範囲をユーザープロセスのページテーブルにマップする
///
/// フレームバッファなどの MMIO 領域をユーザー空間へ公開するために使用する。
/// 新規フレームは割り当てず、指定された物理アドレスのページをそのままマップする。
///
/// ## Arguments
/// * `table_phys` - ユーザープロセスの L4 ページテーブルの物理アドレス
/// * `virt_addr`  - マップ先の仮想アドレス (4KiB アライン済み)
/// * `phys_addr`  - マップ元の物理アドレス (4KiB アライン済み)
/// * `size`       - マップするサイズ (バイト単位)
pub fn map_physical_range_to_user(
    table_phys: u64,
    virt_addr: u64,
    phys_addr: u64,
    size: u64,
) -> Result<()> {
    use crate::result::{Kernel, Memory};
    use x86_64::structures::paging::PageTableFlags as Flags;

    let phys_off = physical_memory_offset().ok_or(Kernel::Memory(Memory::NotMapped))?;
    if size == 0 {
        return Ok(());
    }

    let l4 = unsafe { &mut *((table_phys + phys_off) as *mut PageTable) };
    let mut pt = unsafe { OffsetPageTable::new(l4, VirtAddr::new(phys_off)) };

    let flags = Flags::PRESENT | Flags::WRITABLE | Flags::USER_ACCESSIBLE | Flags::NO_EXECUTE;

    let virt_start = virt_addr & !0xfffu64;
    let phys_start = phys_addr & !0xfffu64;
    let total_pages = size.checked_add(0xfff).map(|v| v >> 12).unwrap_or(0);

    for i in 0..total_pages {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(virt_start + i * 4096));
        let frame = PhysFrame::containing_address(PhysAddr::new(phys_start + i * 4096));

        let map_result = unsafe {
            let mut alloc_lock = frame::FRAME_ALLOCATOR.lock();
            let alloc_ref = alloc_lock
                .as_mut()
                .ok_or(Kernel::Memory(Memory::OutOfMemory))?;
            pt.map_to(page, frame, flags, alloc_ref)
        };

        match map_result {
            Ok(flush) => flush.ignore(),
            Err(x86_64::structures::paging::mapper::MapToError::PageAlreadyMapped(_)) => unsafe {
                if let Ok((_, flush)) = pt.unmap(page) {
                    flush.ignore();
                }
                let mut alloc_lock2 = frame::FRAME_ALLOCATOR.lock();
                let alloc_ref2 = alloc_lock2
                    .as_mut()
                    .ok_or(Kernel::Memory(Memory::OutOfMemory))?;
                pt.map_to(page, frame, flags, alloc_ref2)
                    .map_err(|_| Kernel::Memory(Memory::InvalidAddress))?
                    .ignore();
            },
            Err(_) => return Err(Kernel::Memory(Memory::InvalidAddress)),
        }
    }

    Ok(())
}

/// CR3を指定した物理アドレスのページテーブルに切り替える
///
/// ## Arguments
/// - `table_phys`: 切り替えるページテーブルの物理アドレス
///
/// ## Safety
/// - `table_phys`が有効なページテーブルの物理アドレスであることを呼び出し元が保証する必要がある
pub fn switch_page_table(table_phys: u64) {
    if table_phys == 0 || (table_phys & 0xfff) != 0 {
        crate::warn!(
            "Refusing to switch CR3 with invalid page table address: {:#x}",
            table_phys
        );
        return;
    }
    unsafe {
        let frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(table_phys));
        Cr3::write(frame, Cr3Flags::empty());
    }
}
