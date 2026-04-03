//! 特権システムコール（Service権限プロセス専用）
//!
//! これらのsyscallはPrivilegeLevel::Serviceのプロセスのみ呼び出し可能。
//! 物理メモリ直接操作、ゼロコピーIO等の実装に使用する。

use super::types::{EFAULT, EINVAL, EPERM};
use crate::task::ids::PrivilegeLevel;

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
    if page_count == 0 || page_count > 128 {
        return EINVAL;
    }
    if phys_pages_ptr == 0 {
        return EFAULT;
    }

    // 物理ページアドレス配列をカーネル空間へコピー
    let mut phys_pages = alloc::vec![0u64; page_count as usize];
    let copy_size = page_count as usize * core::mem::size_of::<u64>();
    if let Err(e) = super::copy_from_user(phys_pages_ptr, unsafe {
        core::slice::from_raw_parts_mut(phys_pages.as_mut_ptr() as *mut u8, copy_size)
    }) {
        return e;
    }

    // ターゲットプロセスのページテーブルを取得
    let target_pid = match crate::task::thread_to_process_id(target_thread_id) {
        Some(pid) => pid,
        None => return EINVAL,
    };

    let page_table = match crate::task::with_process(target_pid, |p| p.page_table()) {
        Some(Some(pt)) => pt,
        _ => return EINVAL,
    };

    // 仮想アドレス決定（ヒントがあればそれを使用、なければ自動割り当て）
    let virt_addr = if virt_addr_hint != 0 {
        // アライメントチェック
        if virt_addr_hint & 0xfff != 0 {
            return EINVAL;
        }
        virt_addr_hint
    } else {
        // 自動割り当て: ヒープ上位の領域を使用 (0x6000_0000_0000付近)
        match crate::task::with_process_mut(target_pid, |p| {
            // TODO: プロセス構造体にmmap領域管理を追加して衝突回避
            // 暫定: 固定アドレス + ページカウント
            let base = 0x6000_0000_0000u64;
            base
        }) {
            Some(addr) => addr,
            None => return EINVAL,
        }
    };

    // 各物理ページをマップ
    for (i, &phys_addr) in phys_pages.iter().enumerate() {
        let target_virt = virt_addr + (i as u64 * 0x1000);
        if let Err(_) =
            crate::mem::paging::map_page_in_table(page_table, target_virt, phys_addr, true, true)
        {
            // マップ失敗時はロールバック（既にマップしたページをアンマップ）
            for j in 0..i {
                let rollback_virt = virt_addr + (j as u64 * 0x1000);
                let _ = crate::mem::paging::unmap_page_in_table(page_table, rollback_virt);
            }
            return EFAULT;
        }
    }

    virt_addr
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
/// * arg2: virt_addr_hint - 仮想アドレスのヒント (0=自動割り当て)
///
/// # Returns
/// 成功時: マップされた仮想アドレス
/// エラー時: 負のエラーコード
pub fn alloc_shared_pages(page_count: u64, phys_addrs_out: u64, virt_addr_hint: u64) -> u64 {
    // 権限チェック
    if let Err(e) = require_service_privilege() {
        return e;
    }

    // パラメータ検証
    if page_count == 0 || page_count > 128 {
        return EINVAL;
    }

    // 物理ページを割り当て
    let mut phys_pages = alloc::vec![];
    for _ in 0..page_count {
        match crate::mem::frame::allocate_frame() {
            Ok(frame) => phys_pages.push(frame.start_address().as_u64()),
            Err(_) => {
                // 割り当て失敗時は既に割り当てたページを解放
                for &phys in phys_pages.iter() {
                    use x86_64::{structures::paging::{PhysFrame, Size4KiB}, PhysAddr};
                    if let Some(frame) = PhysFrame::<Size4KiB>::from_start_address(PhysAddr::new(phys)).ok() {
                        let _ = crate::mem::frame::deallocate_frame(frame);
                    }
                }
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
            // 解放
            for &phys in phys_pages.iter() {
                use x86_64::{structures::paging::{PhysFrame, Size4KiB}, PhysAddr};
                if let Some(frame) = PhysFrame::<Size4KiB>::from_start_address(PhysAddr::new(phys)).ok() {
                    let _ = crate::mem::frame::deallocate_frame(frame);
                }
            }
            return EINVAL;
        }
    };

    let page_table = match crate::task::with_process(self_pid, |p| p.page_table()) {
        Some(Some(pt)) => pt,
        _ => {
            // 解放
            for &phys in phys_pages.iter() {
                use x86_64::{structures::paging::{PhysFrame, Size4KiB}, PhysAddr};
                if let Some(frame) = PhysFrame::<Size4KiB>::from_start_address(PhysAddr::new(phys)).ok() {
                    let _ = crate::mem::frame::deallocate_frame(frame);
                }
            }
            return EINVAL;
        }
    };

    // 仮想アドレス決定
    let virt_addr = if virt_addr_hint != 0 {
        if virt_addr_hint & 0xfff != 0 {
            // 解放
            for &phys in phys_pages.iter() {
                let _ = { use x86_64::{structures::paging::{PhysFrame, Size4KiB}, PhysAddr}; if let Some(frame) = PhysFrame::<Size4KiB>::from_start_address(PhysAddr::new(phys)).ok() { let _ = crate::mem::frame::deallocate_frame(frame); } };
            }
            return EINVAL;
        }
        virt_addr_hint
    } else {
        // 自動割り当て: 共有メモリ領域（0x7000_0000_0000付近）を使用
        match crate::task::with_process_mut(self_pid, |p| {
            // TODO: プロセス構造体にmmap領域管理を追加して衝突回避
            let base = 0x7000_0000_0000u64;
            base
        }) {
            Some(addr) => addr,
            None => {
                for &phys in phys_pages.iter() {
                    use x86_64::{structures::paging::{PhysFrame, Size4KiB}, PhysAddr};
                    if let Some(frame) = PhysFrame::<Size4KiB>::from_start_address(PhysAddr::new(phys)).ok() {
                        let _ = crate::mem::frame::deallocate_frame(frame);
                    }
                }
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
            // 物理ページを解放
            for &phys in phys_pages.iter() {
                let _ = { use x86_64::{structures::paging::{PhysFrame, Size4KiB}, PhysAddr}; if let Some(frame) = PhysFrame::<Size4KiB>::from_start_address(PhysAddr::new(phys)).ok() { let _ = crate::mem::frame::deallocate_frame(frame); } };
            }
            return EFAULT;
        }
    }

    // 物理アドレス配列をユーザー空間へ書き込み
    if phys_addrs_out != 0 {
        let copy_size = phys_pages.len() * core::mem::size_of::<u64>();
        if !super::validate_user_ptr(phys_addrs_out, copy_size as u64) {
            // ロールバック
            for i in 0..phys_pages.len() {
                let rollback_virt = virt_addr + (i as u64 * 0x1000);
                let _ = crate::mem::paging::unmap_page_in_table(page_table, rollback_virt);
            }
            for &phys in phys_pages.iter() {
                let _ = { use x86_64::{structures::paging::{PhysFrame, Size4KiB}, PhysAddr}; if let Some(frame) = PhysFrame::<Size4KiB>::from_start_address(PhysAddr::new(phys)).ok() { let _ = crate::mem::frame::deallocate_frame(frame); } };
            }
            return EFAULT;
        }

        super::with_user_memory_access(|| unsafe {
            let dest_slice = core::slice::from_raw_parts_mut(
                phys_addrs_out as *mut u64,
                phys_pages.len(),
            );
            dest_slice.copy_from_slice(&phys_pages);
        });
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
    if page_count == 0 || page_count > 128 {
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
            if let Some(phys) =
                crate::mem::paging::virt_to_phys_in_table(page_table, target_virt)
            {
                phys_addrs.push(phys);
            }
        }
    }

    // アンマップ
    for i in 0..page_count {
        let target_virt = virt_addr + (i * 0x1000);
        let _ = crate::mem::paging::unmap_page_in_table(page_table, target_virt);
    }

    // 物理ページを解放
    if deallocate != 0 {
        for phys in phys_addrs {
            use x86_64::{structures::paging::{PhysFrame, Size4KiB}, PhysAddr};
            let phys_aligned = phys & !0xfff;
            if let Some(frame) = PhysFrame::<Size4KiB>::from_start_address(PhysAddr::new(phys_aligned)).ok() {
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
    if page_count == 0 || page_count > 128 {
        return EINVAL;
    }
    if phys_pages_ptr == 0 {
        return EFAULT;
    }

    // 物理ページアドレス配列をカーネル空間へコピー
    let mut phys_pages = alloc::vec![0u64; page_count as usize];
    let copy_size = page_count as usize * core::mem::size_of::<u64>();
    if let Err(e) = super::copy_from_user(phys_pages_ptr, unsafe {
        core::slice::from_raw_parts_mut(phys_pages.as_mut_ptr() as *mut u8, copy_size)
    }) {
        return e;
    }

    // IPC経由で物理ページリストを送信
    let total_bytes = page_count * 0x1000;
    if super::ipc::send_pages_from_kernel(dest_thread_id, map_start, total_bytes, &phys_pages) {
        0
    } else {
        super::types::EAGAIN
    }
}
