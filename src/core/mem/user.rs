//! ユーザー空間メモリ管理

use spin::Mutex;
use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

use crate::mem::{frame, paging};
use crate::result::{Kernel, Memory, Result};

const PAGE_SIZE: u64 = 4096;
const USER_STACK_TOP: u64 = 0x0000_8000_0000; // 2GB (2^31)
const USER_STACK_GUARD_PAGES: u64 = 1;
const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;

static NEXT_STACK_TOP: Mutex<u64> = Mutex::new(USER_STACK_TOP);

pub struct UserStack {
    pub bottom: u64,
    pub top: u64,
}

fn current_process_user_page_table() -> Result<u64> {
    let pid = crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |thread| thread.process_id()))
        .ok_or(Kernel::Memory(Memory::NotMapped))?;
    crate::task::with_process(pid, |proc| proc.page_table())
        .flatten()
        .ok_or(Kernel::Memory(Memory::NotMapped))
}

/// 指定したユーザーページテーブル上に任意のユーザ空間レンジをマップ
pub fn map_user_range_in_table(
    table_phys: u64,
    start: u64,
    size: u64,
    flags: PageTableFlags,
) -> Result<()> {
    if size == 0 {
        return Ok(());
    }
    let size_minus_one = size
        .checked_sub(1)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;
    let end = start
        .checked_add(size_minus_one)
        .ok_or(Kernel::Memory(Memory::InvalidAddress))?;
    if start == 0 || start > USER_SPACE_END || end > USER_SPACE_END {
        return Err(Kernel::Memory(Memory::InvalidAddress));
    }
    if !flags.contains(PageTableFlags::USER_ACCESSIBLE) || !flags.contains(PageTableFlags::PRESENT)
    {
        return Err(Kernel::Memory(Memory::PermissionDenied));
    }

    paging::map_and_copy_segment_to(
        table_phys,
        start,
        0,
        size,
        &[],
        flags.contains(PageTableFlags::WRITABLE),
        !flags.contains(PageTableFlags::NO_EXECUTE),
    )
}

/// 任意のユーザ空間レンジをマップ
pub fn map_user_range(start: u64, size: u64, flags: PageTableFlags) -> Result<()> {
    let table_phys = current_process_user_page_table()?;
    map_user_range_in_table(table_phys, start, size, flags)
}

/// ユーザスタックを確保
pub fn alloc_user_stack(pages: u64) -> Result<UserStack> {
    let table_phys = current_process_user_page_table()?;
    alloc_user_stack_in_table(table_phys, pages)
}

/// 指定したユーザーページテーブル上にユーザスタックを確保
pub fn alloc_user_stack_in_table(table_phys: u64, pages: u64) -> Result<UserStack> {
    if pages == 0 {
        return Err(Kernel::Memory(Memory::InvalidAddress));
    }

    let mut top = NEXT_STACK_TOP.lock();

    let stack_size = pages * PAGE_SIZE;
    let total = stack_size + USER_STACK_GUARD_PAGES * PAGE_SIZE;

    let new_top = top
        .checked_sub(total)
        .ok_or(Kernel::Memory(Memory::OutOfMemory))?;

    let stack_bottom = new_top + USER_STACK_GUARD_PAGES * PAGE_SIZE;
    let stack_top = new_top + total;

    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;

    map_user_range_in_table(table_phys, stack_bottom, stack_size, flags)?;

    *top = new_top;

    Ok(UserStack {
        bottom: stack_bottom,
        top: stack_top,
    })
}
