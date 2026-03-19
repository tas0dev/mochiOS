use super::types::{EFAULT, EINVAL, ENOMEM, EPERM};
use x86_64::VirtAddr;

const MAX_MMIO_MAP_SIZE: u64 = 64 * 1024 * 1024;

fn caller_has_mmio_privilege() -> bool {
    crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
        .and_then(|pid| {
            crate::task::with_process(pid, |p| {
                matches!(
                    p.privilege(),
                    crate::task::PrivilegeLevel::Core | crate::task::PrivilegeLevel::Service
                )
            })
        })
        .unwrap_or(false)
}

fn current_process_page_table() -> Option<u64> {
    crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
        .and_then(|pid| crate::task::with_process(pid, |p| p.page_table()))
        .flatten()
}

/// 物理アドレス範囲を呼び出し元プロセスへマップする
///
/// # Returns
/// 成功時: マップ済みユーザー仮想アドレス
/// 失敗時: errno
pub fn map_physical_range(phys_addr: u64, size: u64) -> u64 {
    if !caller_has_mmio_privilege() {
        return EPERM;
    }
    if size == 0 {
        return EINVAL;
    }

    let aligned_phys = phys_addr & !0xfffu64;
    let page_offset = phys_addr & 0xfffu64;
    let mapped_size = match size
        .checked_add(page_offset)
        .and_then(|v| v.checked_add(0xfff))
        .map(|v| v & !0xfffu64)
    {
        Some(v) if v != 0 => v,
        _ => return EINVAL,
    };
    if mapped_size > MAX_MMIO_MAP_SIZE {
        return EINVAL;
    }

    let tid = match crate::task::current_thread_id() {
        Some(t) => t,
        None => return ENOMEM,
    };
    let pid = match crate::task::with_thread(tid, |t| t.process_id()) {
        Some(p) => p,
        None => return ENOMEM,
    };

    let result = crate::task::with_process_mut(pid, |process| {
        if process.heap_start() == 0 {
            let default_base = 0x5000_0000u64;
            process.set_heap_start(default_base);
            process.set_heap_end(default_base);
        }

        let base = process.heap_end();
        let map_start = base.checked_add(0xfff).map(|v| v & !0xfffu64).unwrap_or(0);
        if map_start == 0 || map_start > 0x0000_7FFF_FFFF_FFFF {
            return Err(ENOMEM);
        }

        let pt_phys = match process.page_table() {
            Some(p) => p,
            None => return Err(ENOMEM),
        };

        crate::mem::paging::map_physical_range_to_user(
            pt_phys,
            map_start,
            aligned_phys,
            mapped_size,
        )
        .map_err(|_| ENOMEM)?;

        let new_end = map_start.checked_add(mapped_size).unwrap_or(map_start);
        process.set_heap_end(new_end);

        map_start.checked_add(page_offset).ok_or(ENOMEM)
    });

    match result {
        Some(Ok(va)) => va,
        Some(Err(e)) => e,
        None => ENOMEM,
    }
}

/// ユーザー仮想アドレスを物理アドレスへ変換する
///
/// xHCI など DMA デバイスに渡すアドレス算出で使用する。
pub fn virt_to_phys(user_vaddr: u64) -> u64 {
    if !caller_has_mmio_privilege() {
        return EPERM;
    }
    if user_vaddr == 0 {
        return EFAULT;
    }
    if !crate::syscall::validate_user_ptr(user_vaddr, 1) {
        return EFAULT;
    }

    let table_phys = match current_process_page_table() {
        Some(pt) => pt,
        None => return ENOMEM,
    };

    match crate::mem::paging::translate_addr_in_table(table_phys, VirtAddr::new(user_vaddr)) {
        Some((phys, _)) => phys.as_u64(),
        None => EFAULT,
    }
}
