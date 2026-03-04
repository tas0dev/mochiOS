//! ユーザー空間メモリ管理

use spin::Mutex;
use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

use crate::error::{KernelError, MemoryError, Result};
use crate::mem::{frame, paging};

const PAGE_SIZE: u64 = 4096;
const USER_STACK_TOP: u64 = 0x0000_8000_0000; // 2GB
const USER_STACK_GUARD_PAGES: u64 = 1;
const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;

static NEXT_STACK_TOP: Mutex<u64> = Mutex::new(USER_STACK_TOP);

pub struct UserStack {
    pub bottom: u64,
    pub top: u64,
}

/// 任意のユーザ空間レンジをマップ
pub fn map_user_range(start: u64, size: u64, flags: PageTableFlags) -> Result<()> {
    if size == 0 {
        return Ok(());
    }
    let size_minus_one = size
        .checked_sub(1)
        .ok_or(KernelError::Memory(MemoryError::InvalidAddress))?;
    let end = start
        .checked_add(size_minus_one)
        .ok_or(KernelError::Memory(MemoryError::InvalidAddress))?;
    if start == 0 || start > USER_SPACE_END || end > USER_SPACE_END {
        return Err(KernelError::Memory(MemoryError::InvalidAddress));
    }

    let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(start));
    let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end));

    for page in Page::range_inclusive(start_page, end_page) {
        let frame = frame::allocate_frame()?;
        paging::map_page(page, frame, flags)?;
    }

    Ok(())
}

/// ユーザスタックを確保
pub fn alloc_user_stack(pages: u64) -> Result<UserStack> {
    if pages == 0 {
        return Err(KernelError::Memory(MemoryError::InvalidAddress));
    }

    let mut top = NEXT_STACK_TOP.lock();

    let stack_size = pages * PAGE_SIZE;
    let total = stack_size + USER_STACK_GUARD_PAGES * PAGE_SIZE;

    let new_top = top
        .checked_sub(total)
        .ok_or(KernelError::Memory(MemoryError::OutOfMemory))?;

    let stack_bottom = new_top + USER_STACK_GUARD_PAGES * PAGE_SIZE;
    let stack_top = new_top + total;

    // NO_EXECUTE フラグを設定してスタックの実行を禁止する (MED-03)
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;

    map_user_range(stack_bottom, stack_size, flags)?;

    *top = new_top;

    Ok(UserStack {
        bottom: stack_bottom,
        top: stack_top,
    })
}
