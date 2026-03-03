//! ページング管理モジュール
//!
//! 仮想メモリとページテーブル管理

use crate::info;
use crate::mem::frame;
use crate::result::{Kernel, Memory, Result};
use spin::Mutex;
use uefi::table::boot::MemoryType as UefiMemoryType;
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

fn protect_kernel_text_pages(page_table: &mut OffsetPageTable<'static>) {
    // リンカ依存を避けるため、現在実行中コードページを最小限RO化する
    let rip: u64;
    unsafe {
        core::arch::asm!("lea {}, [rip]", out(reg) rip);
    }
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(rip & !0xfffu64));
    unsafe {
        let _ = page_table
            .update_flags(page, PageTableFlags::PRESENT)
            .map(|flush| flush.flush());
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

/// 物理メモリオフセットを取得
///
/// ## Returns
/// カーネルが使用する物理メモリオフセット（仮想アドレス = 物理アドレス + オフセット）
pub fn physical_memory_offset() -> Option<u64> {
    *PHYS_OFFSET.lock()
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
                let phys_frame =
                    PhysFrame::containing_address(x86_64::PhysAddr::new(phys_frame_addr));
                {
                    let mut alloc_lock = crate::mem::frame::FRAME_ALLOCATOR.lock();
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

    // L4[0]: カーネルコード/スタックとユーザーコードが共存するエントリ
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
            let kernel_l2 = unsafe { &*((kernel_l2_phys + phys_off) as *const PageTable) };

            let new_l2_frame = frame::allocate_frame()?;
            let new_l2_phys = new_l2_frame.start_address().as_u64();
            let new_l2 = unsafe { &mut *((new_l2_phys + phys_off) as *mut PageTable) };
            new_l2.zero();

            // カーネルのL2をすべてコピー（L2[0] = 0-2MB の恒等マップも含む）
            // これによりsyscall中にカーネルが低アドレス物理フレームにアクセスできる
            for i in 0..512 {
                new_l2[i] = kernel_l2[i].clone();
            }
            // L2[4] = 0x800000-0x9FFFFF はユーザーコード専用にクリア（exec時に再マップ）
            new_l2[4].set_unused();

            new_l3[0].set_addr(x86_64::PhysAddr::new(new_l2_phys), kernel_l3[0].flags());
        }

        // L3[1..512]: 1GB以上のカーネルメモリをそのままコピー
        for i in 1..512 {
            new_l3[i] = kernel_l3[i].clone();
        }

        new_l4[0].set_addr(x86_64::PhysAddr::new(new_l3_phys), kernel_l4[0].flags());
    }

    // L4[1..255]: その他の物理メモリ領域をカーネルからコピー
    for i in 1..255 {
        new_l4[i] = kernel_l4[i].clone();
    }
    // L4[255]: ユーザースタック領域（プロセス固有 - 空のままにする）
    // L4[256..512]: カーネル上位半分をカーネルからコピー
    for i in 256..512 {
        new_l4[i] = kernel_l4[i].clone();
    }

    Ok(new_l4_phys)
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
        let phys_frame_addr = frame.start_address().as_u64();

        // フレームを先にゼロ初期化（BSS領域のため）
        unsafe {
            core::ptr::write_bytes((phys_frame_addr + phys_off) as *mut u8, 0, 4096);
        }

        // マップ（既にマップ済みの場合はアンマップして再マップ）
        unsafe {
            let mut alloc_lock = frame::FRAME_ALLOCATOR.lock();
            let alloc_ref = alloc_lock
                .as_mut()
                .ok_or(Kernel::Memory(Memory::OutOfMemory))?;
            match pt.map_to(page, frame, final_flags, alloc_ref) {
                Ok(flush) => {
                    flush.ignore();
                }
                Err(x86_64::structures::paging::mapper::MapToError::PageAlreadyMapped(_)) => {
                    // カーネルのアイデンティティマップが残っている場合：アンマップして再マップ
                    let (old_frame, flush) = pt
                        .unmap(page)
                        .map_err(|_| Kernel::Memory(Memory::InvalidAddress))?;
                    flush.ignore();
                    let _ = frame::deallocate_frame(old_frame);
                    let mut alloc_lock2 = frame::FRAME_ALLOCATOR.lock();
                    let alloc_ref2 = alloc_lock2
                        .as_mut()
                        .ok_or(Kernel::Memory(Memory::OutOfMemory))?;
                    pt.map_to(page, frame, final_flags, alloc_ref2)
                        .map_err(|_| Kernel::Memory(Memory::InvalidAddress))?
                        .ignore();
                }
                Err(_) => return Err(Kernel::Memory(Memory::InvalidAddress)),
            }
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
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_addr));
        if let Ok((frame, flush)) = pt.unmap(page) {
            flush.ignore();
            let _ = frame::deallocate_frame(frame);
        }
        page_addr += 4096;
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
    unsafe {
        let frame = PhysFrame::<Size4KiB>::containing_address(x86_64::PhysAddr::new(table_phys));
        Cr3::write(frame, Cr3Flags::empty());
    }
}
