//! 特権システムコール（Service権限プロセス専用）

use super::sys::{syscall2, syscall3, syscall4, SyscallNumber};

/// 物理ページ配列をターゲットプロセスのアドレス空間にマップ
///
/// **Service権限専用**: PrivilegeLevel::Service以外から呼び出すとEPERMを返す
///
/// # Arguments
/// * `target_thread_id` - マップ先のスレッドID
/// * `phys_pages` - 物理ページアドレス配列 (各要素は4KBページの物理アドレス)
/// * `virt_addr_hint` - 仮想アドレスのヒント (0=自動割り当て)
///
/// # Returns
/// 成功時: マップされた仮想アドレス
/// エラー時: 負のエラーコード（u64としてキャスト）
///
/// # Safety
/// - 物理ページアドレスが有効であることを呼び出し側で保証する必要がある
/// - マップされたメモリへのアクセスは適切な同期が必要
pub fn map_physical_pages(
    target_thread_id: u64,
    phys_pages: &[u64],
    virt_addr_hint: u64,
) -> u64 {
    syscall4(
        SyscallNumber::MapPhysicalPages as u64,
        target_thread_id,
        phys_pages.as_ptr() as u64,
        phys_pages.len() as u64,
        virt_addr_hint,
    )
}

/// 仮想アドレスから物理アドレスを取得
///
/// **Service権限専用**: PrivilegeLevel::Service以外から呼び出すとEPERMを返す
///
/// # Arguments
/// * `virt_addr` - 変換したい仮想アドレス
/// * `target_thread_id` - 対象スレッドID (0=自プロセス)
///
/// # Returns
/// 成功時: 物理アドレス
/// エラー時: 負のエラーコード（u64としてキャスト）
pub fn get_physical_addr(virt_addr: u64, target_thread_id: u64) -> u64 {
    syscall2(
        SyscallNumber::GetPhysicalAddr as u64,
        virt_addr,
        target_thread_id,
    )
}

/// 共有用物理ページを割り当て、自プロセスにマップして物理アドレスを返す
///
/// **Service権限専用**: PrivilegeLevel::Service以外から呼び出すとEPERMを返す
///
/// # Arguments
/// * `page_count` - 割り当てるページ数（最大128）
/// * `phys_addrs_out` - 物理アドレス配列を書き込むバッファ（Option、Noneなら書き込まない）
/// * `virt_addr_hint` - 仮想アドレスのヒント (0=自動割り当て)
///
/// # Returns
/// 成功時: マップされた仮想アドレス
/// エラー時: 負のエラーコード（u64としてキャスト）
///
/// # Safety
/// - 返された仮想アドレス範囲は適切に管理する必要がある
/// - 使用後は `unmap_pages` で解放すること
pub fn alloc_shared_pages(
    page_count: u64,
    phys_addrs_out: Option<&mut [u64]>,
    virt_addr_hint: u64,
) -> u64 {
    let ptr = phys_addrs_out
        .map(|s| s.as_mut_ptr() as u64)
        .unwrap_or(0);
    syscall3(
        SyscallNumber::AllocSharedPages as u64,
        page_count,
        ptr,
        virt_addr_hint,
    )
}

/// 物理ページをアンマップして解放
///
/// **Service権限専用**: PrivilegeLevel::Service以外から呼び出すとEPERMを返す
///
/// # Arguments
/// * `virt_addr` - アンマップする仮想アドレス（ページ境界）
/// * `page_count` - アンマップするページ数
/// * `deallocate` - true=物理ページも解放、false=アンマップのみ
///
/// # Returns
/// 成功時: 0
/// エラー時: 負のエラーコード（u64としてキャスト）
pub fn unmap_pages(virt_addr: u64, page_count: u64, deallocate: bool) -> u64 {
    syscall3(
        SyscallNumber::UnmapPages as u64,
        virt_addr,
        page_count,
        if deallocate { 1 } else { 0 },
    )
}

/// IPC経由で物理ページをターゲットプロセスへ送信
///
/// **Service権限専用**: PrivilegeLevel::Service以外から呼び出すとEPERMを返す
///
/// # Arguments
/// * `dest_thread_id` - 送信先スレッドID
/// * `phys_pages` - 物理ページアドレス配列
/// * `map_start` - マップ先の仮想アドレスヒント (0=自動)
///
/// # Returns
/// 成功時: 0
/// エラー時: 負のエラーコード（u64としてキャスト）
///
/// # Safety
/// - 送信先プロセスが存在すること
/// - 物理ページが有効であること
pub fn ipc_send_pages(dest_thread_id: u64, phys_pages: &[u64], map_start: u64) -> u64 {
    syscall4(
        SyscallNumber::IpcSendPages as u64,
        dest_thread_id,
        phys_pages.as_ptr() as u64,
        phys_pages.len() as u64,
        map_start,
    )
}
