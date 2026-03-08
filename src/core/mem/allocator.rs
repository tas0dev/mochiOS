use linked_list_allocator::LockedHeap;
use x86_64::{
    structures::paging::{
        mapper::MapToError, FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};

/// 仮想アドレス空間のどこからヒープを開始するか
pub const HEAP_START: usize = 0x_4444_4444_0000;
/// ヒープのサイズ
pub const HEAP_SIZE: usize = 32 * 1024 * 1024; // 32 MiB

/// ヒープを初期化
///
/// ## Arguments
/// - `mapper`: 仮想アドレスと物理アドレスのマッピングを管理するオブジェクト
/// - `frame_allocator`: 物理フレームの割り当てを管理するオブジェクト
/// - `heap_allocator_ptr`: ヒープアロケータのロックされたヒープへのポインタ
///
/// ## Returns
/// - `Ok(())` ヒープの初期化に成功した場合
/// - `Err(MapToError<Size4KiB>)` マッピングのエラーが発生した場合
pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    heap_allocator_ptr: u64,
) -> Result<(), MapToError<Size4KiB>> {
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + HEAP_SIZE as u64 - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    // ヒープの仮想アドレス空間を物理フレームにマッピング
    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        // カーネルヒープは実行不可（W^X: NO_EXECUTE でコード実行を防ぐ）
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
        unsafe {
            mapper.map_to(page, frame, flags, frame_allocator)?.flush();
        }
    }

    // ヒープアロケータを初期化
    unsafe {
        let allocator = &mut *(heap_allocator_ptr as *mut LockedHeap);
        allocator.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }

    Ok(())
}

#[alloc_error_handler]
fn alloc_error_handler(layout: alloc::alloc::Layout) -> ! {
    crate::warn!("allocation error: {:?}", layout);
    // スケジューラが動作中でカレントスレッドがあれば、そのプロセスを終了して回復を試みる
    if crate::task::scheduler::is_scheduler_enabled() && crate::task::current_thread_id().is_some()
    {
        crate::warn!("OOM: terminating current process to recover");
        crate::task::scheduler::exit_current_process(-1);
    }
    // 回復不能: 割り込みを無効化してシステムを停止
    #[cfg(target_arch = "x86_64")]
    unsafe {
        x86_64::instructions::interrupts::disable();
    }
    loop {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}
