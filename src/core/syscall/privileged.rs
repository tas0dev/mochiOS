//! 特権システムコール（Service権限プロセス専用）
//!
//! これらのsyscallはPrivilegeLevel::Serviceのプロセスのみ呼び出し可能。
//! 物理メモリ直接操作、ゼロコピーIO等の実装に使用する。

use super::types::{EFAULT, EINVAL, EPERM};
use crate::task::ids::PrivilegeLevel;
use alloc::vec::Vec;
use x86_64::instructions::tlb;
use x86_64::VirtAddr;

/// 現在スレッドのプロセス権限レベルを取得
fn current_process_privilege() -> Option<PrivilegeLevel> {
    crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
        .and_then(|pid| crate::task::with_process(pid, |p| p.privilege()))
}

/// Service権限チェック
fn require_service_privilege() -> Result<(), u64> {
    match current_process_privilege() {
        Some(PrivilegeLevel::Core) | Some(PrivilegeLevel::Service) => Ok(()),
        _ => Err(EPERM),
    }
}

fn deallocate_frames(phys_addrs: &[u64]) {
    for &phys in phys_addrs {
        use x86_64::{
            structures::paging::{PhysFrame, Size4KiB},
            PhysAddr,
        };
        if let Some(frame) = PhysFrame::<Size4KiB>::from_start_address(PhysAddr::new(phys)).ok() {
            let _ = crate::mem::frame::deallocate_frame(frame);
        }
    }
}

fn is_allowed_phys_page(phys_addr: u64) -> bool {
    (phys_addr & 0xfff) == 0 && crate::mem::frame::is_usable_physical_address(phys_addr)
}

fn shared_page_limit() -> u64 {
    crate::mem::frame::get_memory_info()
        .map(|(_, frames)| (frames as u64).max(128))
        .unwrap_or(128)
}

fn map_phys_pages_into_target(
    target_thread_id: u64,
    phys_pages: &[u64],
    virt_addr_hint: u64,
) -> Result<u64, u64> {
    if phys_pages.is_empty() || phys_pages.len() as u64 > shared_page_limit() {
        return Err(EINVAL);
    }
    for &phys_addr in phys_pages {
        if !is_allowed_phys_page(phys_addr) {
            return Err(EINVAL);
        }
    }

    let target_pid = crate::task::thread_to_process_id(target_thread_id).ok_or(EINVAL)?;
    let page_span = (phys_pages.len() as u64)
        .checked_mul(0x1000)
        .ok_or(EINVAL)?;
    let (virt_addr, page_table, reserved_heap_old, reserved_heap_new) = if virt_addr_hint != 0 {
        if virt_addr_hint & 0xfff != 0 {
            return Err(EINVAL);
        }
        let pt = crate::task::with_process(target_pid, |p| p.page_table())
            .flatten()
            .ok_or(EINVAL)?;
        (virt_addr_hint, pt, None, None)
    } else {
        let (virt_addr, pt, old_end, new_end) = crate::task::with_process_mut(target_pid, |p| {
            let base = if p.heap_end() == 0 {
                0x6000_0000_0000u64
            } else {
                p.heap_end()
            };
            let virt_addr = base
                .checked_add(0xfff)
                .map(|v| v & !0xfffu64)
                .ok_or(EINVAL)?;
            let new_end = virt_addr.checked_add(page_span).ok_or(EINVAL)?;
            let pt = p.page_table().ok_or(EINVAL)?;
            let old_end = p.heap_end();
            p.set_heap_end(new_end);
            Ok::<(u64, u64, u64, u64), u64>((virt_addr, pt, old_end, new_end))
        })
        .ok_or(EINVAL)??;
        (virt_addr, pt, Some(old_end), Some(new_end))
    };

    for (i, &phys_addr) in phys_pages.iter().enumerate() {
        let target_virt = virt_addr + (i as u64 * 0x1000);
        if crate::mem::paging::map_page_in_table(page_table, target_virt, phys_addr, true, true)
            .is_err()
        {
            for j in 0..i {
                let rollback_virt = virt_addr + (j as u64 * 0x1000);
                let _ = crate::mem::paging::unmap_page_in_table(page_table, rollback_virt);
            }
            if let (Some(old_end), Some(new_end)) = (reserved_heap_old, reserved_heap_new) {
                let _ = crate::task::with_process_mut(target_pid, |p| {
                    if p.heap_end() == new_end {
                        p.set_heap_end(old_end);
                    }
                });
            }
            return Err(EFAULT);
        }
    }

    Ok(virt_addr)
}

/// 物理ページ配列をターゲットプロセスのアドレス空間にマップ
///
/// # Arguments
/// * arg0: target_thread_id - マップ先のスレッドID
/// * arg1: phys_pages_ptr - 物理ページアドレス配列へのポインタ (u64配列)
/// * arg2: page_count - ページ数
/// * arg3: virt_addr_hint - 仮想アドレスのヒント (0=自動割り当て)
///
/// # Returns
/// 成功時: マップされた仮想アドレス
/// エラー時: 負のエラーコード
pub fn map_physical_pages(
    target_thread_id: u64,
    phys_pages_ptr: u64,
    page_count: u64,
    virt_addr_hint: u64,
) -> u64 {
    // 権限チェック
    if let Err(e) = require_service_privilege() {
        return e;
    }

    // パラメータ検証
    if page_count == 0 || page_count > shared_page_limit() {
        return EINVAL;
    }
    if phys_pages_ptr == 0 {
        return EFAULT;
    }

    let mut phys_pages = alloc::vec![0u64; page_count as usize];
    for i in 0..page_count as usize {
        let addr = match phys_pages_ptr.checked_add((i * core::mem::size_of::<u64>()) as u64) {
            Some(addr) => addr,
            None => return EFAULT,
        };
        match super::read_user_u64(addr) {
            Ok(page) => phys_pages[i] = page,
            Err(e) => return e,
        }
    }

    match map_phys_pages_into_target(target_thread_id, &phys_pages, virt_addr_hint) {
        Ok(v) => v,
        Err(e) => e,
    }
}

/// 仮想アドレスから物理アドレスを取得（Service権限強化版）
///
/// # Arguments
/// * arg0: virt_addr - 仮想アドレス
/// * arg1: target_thread_id - 対象スレッドID (0=自プロセス)
///
/// # Returns
/// 成功時: 物理アドレス
/// エラー時: 負のエラーコード
pub fn get_physical_addr(virt_addr: u64, target_thread_id: u64) -> u64 {
    // 権限チェック
    if let Err(e) = require_service_privilege() {
        return e;
    }

    if virt_addr == 0 {
        return EINVAL;
    }

    let pid = if target_thread_id == 0 {
        // 自プロセス
        match crate::task::current_thread_id()
            .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
        {
            Some(pid) => pid,
            None => return EINVAL,
        }
    } else {
        // 指定スレッドのプロセス
        match crate::task::thread_to_process_id(target_thread_id) {
            Some(pid) => pid,
            None => return EINVAL,
        }
    };

    let page_table = match crate::task::with_process(pid, |p| p.page_table()) {
        Some(Some(pt)) => pt,
        _ => return EINVAL,
    };

    match crate::mem::paging::virt_to_phys_in_table(page_table, virt_addr) {
        Some(phys) => phys,
        None => EFAULT,
    }
}

/// 共有用物理ページを割り当て、自プロセスにマップして物理アドレスを返す
///
/// # Arguments
/// * arg0: page_count - 割り当てるページ数
/// * arg1: phys_addrs_out - 物理アドレス配列を書き込むユーザー空間バッファ (u64配列)
/// * arg2: phys_addrs_len - arg1 バッファの要素数
/// * arg3: virt_addr_hint - 仮想アドレスのヒント (0=自動割り当て)
///
/// # Returns
/// 成功時: マップされた仮想アドレス
/// エラー時: 負のエラーコード
pub fn alloc_shared_pages(
    page_count: u64,
    phys_addrs_out: u64,
    phys_addrs_len: u64,
    virt_addr_hint: u64,
) -> u64 {
    // 権限チェック
    if let Err(e) = require_service_privilege() {
        return e;
    }

    // パラメータ検証
    if page_count == 0 || page_count > shared_page_limit() {
        return EINVAL;
    }
    if phys_addrs_out != 0 && phys_addrs_len < page_count {
        return EINVAL;
    }

    // 物理ページを割り当て
    let mut phys_pages = alloc::vec![];
    for _ in 0..page_count {
        match crate::mem::frame::allocate_frame() {
            Ok(frame) => phys_pages.push(frame.start_address().as_u64()),
            Err(_) => {
                deallocate_frames(&phys_pages);
                return super::types::ENOMEM;
            }
        }
    }

    // 自プロセスのページテーブルを取得
    let self_pid = match crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
    {
        Some(pid) => pid,
        None => {
            deallocate_frames(&phys_pages);
            return EINVAL;
        }
    };

    let page_span = match page_count.checked_mul(0x1000) {
        Some(v) => v,
        None => {
            deallocate_frames(&phys_pages);
            return EINVAL;
        }
    };

    // 仮想アドレス決定
    let (virt_addr, page_table, reserved_heap_old, reserved_heap_new) = if virt_addr_hint != 0 {
        if virt_addr_hint & 0xfff != 0 {
            deallocate_frames(&phys_pages);
            return EINVAL;
        }
        let pt = match crate::task::with_process(self_pid, |p| p.page_table()) {
            Some(Some(v)) => v,
            _ => {
                deallocate_frames(&phys_pages);
                return EINVAL;
            }
        };
        (virt_addr_hint, pt, None, None)
    } else {
        // 自動割り当て: alloc_shared_pages は「自己プロセス内共有ページ」向けに
        // 0x7000_0000_0000 帯を使用する。
        match crate::task::with_process_mut(self_pid, |p| {
            let base = if p.heap_end() == 0 {
                0x7000_0000_0000u64
            } else {
                p.heap_end()
            };
            let virt_addr = base
                .checked_add(0xfff)
                .map(|v| v & !0xfffu64)
                .ok_or(EINVAL)?;
            let new_end = virt_addr.checked_add(page_span).ok_or(EINVAL)?;
            let pt = p.page_table().ok_or(EINVAL)?;
            let old_end = p.heap_end();
            p.set_heap_end(new_end);
            Ok((virt_addr, pt, old_end, new_end))
        }) {
            Some(Ok(v)) => (v.0, v.1, Some(v.2), Some(v.3)),
            Some(Err(e)) => {
                deallocate_frames(&phys_pages);
                return e;
            }
            None => {
                deallocate_frames(&phys_pages);
                return EINVAL;
            }
        }
    };

    // 各物理ページをマップ
    for (i, &phys_addr) in phys_pages.iter().enumerate() {
        let target_virt = virt_addr + (i as u64 * 0x1000);
        if let Err(_) =
            crate::mem::paging::map_page_in_table(page_table, target_virt, phys_addr, true, true)
        {
            // マップ失敗時はロールバック
            for j in 0..i {
                let rollback_virt = virt_addr + (j as u64 * 0x1000);
                let _ = crate::mem::paging::unmap_page_in_table(page_table, rollback_virt);
            }
            if let (Some(old_end), Some(new_end)) = (reserved_heap_old, reserved_heap_new) {
                let _ = crate::task::with_process_mut(self_pid, |p| {
                    if p.heap_end() == new_end {
                        p.set_heap_end(old_end);
                    }
                });
            }
            deallocate_frames(&phys_pages);
            return EFAULT;
        }
    }

    // 物理アドレス配列をユーザー空間へ書き込み
    if phys_addrs_out != 0 {
        let mut copy_bytes = alloc::vec![0u8; phys_pages.len() * core::mem::size_of::<u64>()];
        for (i, page) in phys_pages.iter().enumerate() {
            let off = i * core::mem::size_of::<u64>();
            copy_bytes[off..off + 8].copy_from_slice(&page.to_ne_bytes());
        }
        if let Err(errno) = super::copy_to_user(phys_addrs_out, &copy_bytes) {
            // ロールバック
            for i in 0..phys_pages.len() {
                let rollback_virt = virt_addr + (i as u64 * 0x1000);
                let _ = crate::mem::paging::unmap_page_in_table(page_table, rollback_virt);
            }
            if let (Some(old_end), Some(new_end)) = (reserved_heap_old, reserved_heap_new) {
                let _ = crate::task::with_process_mut(self_pid, |p| {
                    if p.heap_end() == new_end {
                        p.set_heap_end(old_end);
                    }
                });
            }
            deallocate_frames(&phys_pages);
            return errno;
        }
    }

    virt_addr
}

/// 物理ページをアンマップして解放
///
/// # Arguments
/// * arg0: virt_addr - アンマップする仮想アドレス（ページ境界）
/// * arg1: page_count - アンマップするページ数
/// * arg2: deallocate - 1=物理ページも解放、0=アンマップのみ
///
/// # Returns
/// 成功時: 0
/// エラー時: 負のエラーコード
pub fn unmap_pages(virt_addr: u64, page_count: u64, deallocate: u64) -> u64 {
    // 権限チェック
    if let Err(e) = require_service_privilege() {
        return e;
    }

    // パラメータ検証
    if page_count == 0 || page_count > shared_page_limit() {
        return EINVAL;
    }
    if virt_addr & 0xfff != 0 {
        return EINVAL;
    }

    // 自プロセスのページテーブルを取得
    let self_pid = match crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
    {
        Some(pid) => pid,
        None => return EINVAL,
    };

    let page_table = match crate::task::with_process(self_pid, |p| p.page_table()) {
        Some(Some(pt)) => pt,
        _ => return EINVAL,
    };

    // 物理アドレスを取得してからアンマップ
    let mut phys_addrs = alloc::vec![];
    if deallocate != 0 {
        for i in 0..page_count {
            let target_virt = virt_addr + (i * 0x1000);
            if let Some(phys) = crate::mem::paging::virt_to_phys_in_table(page_table, target_virt) {
                phys_addrs.push(phys);
            }
        }
    }

    // アンマップ
    for i in 0..page_count {
        let target_virt = virt_addr + (i * 0x1000);
        let _ = crate::mem::paging::unmap_page_in_table(page_table, target_virt);
        if let Ok(vaddr) = VirtAddr::try_new(target_virt) {
            tlb::flush(vaddr);
        }
    }

    // 物理ページを解放
    if deallocate != 0 {
        let mut freed: Vec<u64> = Vec::new();
        for phys in phys_addrs {
            use x86_64::{
                structures::paging::{PhysFrame, Size4KiB},
                PhysAddr,
            };
            let phys_aligned = phys & !0xfff;
            if freed.iter().any(|p| *p == phys_aligned) {
                continue;
            }
            freed.push(phys_aligned);
            if let Some(frame) =
                PhysFrame::<Size4KiB>::from_start_address(PhysAddr::new(phys_aligned)).ok()
            {
                let _ = crate::mem::frame::deallocate_frame(frame);
            }
        }
    }

    0
}

/// IPC経由で物理ページをターゲットプロセスへ送信
///
/// # Arguments
/// * arg0: dest_thread_id - 送信先スレッドID
/// * arg1: phys_pages_ptr - 物理ページアドレス配列へのポインタ (u64配列)
/// * arg2: page_count - ページ数
/// * arg3: map_start - マップ先の仮想アドレスヒント (0=自動)
///
/// # Returns
/// 成功時: 0
/// エラー時: 負のエラーコード
pub fn ipc_send_pages(
    dest_thread_id: u64,
    phys_pages_ptr: u64,
    page_count: u64,
    map_start: u64,
) -> u64 {
    // 権限チェック
    if let Err(e) = require_service_privilege() {
        return e;
    }

    // パラメータ検証
    if dest_thread_id == 0 {
        return EINVAL;
    }
    if page_count == 0 || page_count > shared_page_limit() {
        return EINVAL;
    }
    if phys_pages_ptr == 0 {
        return EFAULT;
    }

    let mut phys_pages = alloc::vec![0u64; page_count as usize];
    for i in 0..page_count as usize {
        let addr = match phys_pages_ptr.checked_add((i * core::mem::size_of::<u64>()) as u64) {
            Some(addr) => addr,
            None => return EFAULT,
        };
        match super::read_user_u64(addr) {
            Ok(page) => phys_pages[i] = page,
            Err(e) => return e,
        }
    }

    let mapped_addr = match map_phys_pages_into_target(dest_thread_id, &phys_pages, map_start) {
        Ok(addr) => addr,
        Err(e) => return e,
    };
    let total_bytes = page_count * 0x1000;
    if super::ipc::send_map_header_from_kernel(dest_thread_id, mapped_addr, total_bytes) {
        0
    } else {
        super::types::EAGAIN
    }
}
