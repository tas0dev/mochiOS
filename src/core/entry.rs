#![no_std]
#![no_main]

//! カーネルスタンドアローンバイナリのエントリポイント
//!
//! ブートローダーは sysv64 呼び出し規約で kernel_entry(boot_info_ptr) を呼ぶ。
//! ここで自前の LockedHeap アロケータを設定してから mochios のカーネル本体へ移譲する。

extern crate alloc;

use linked_list_allocator::LockedHeap;

/// カーネルのグローバルアロケータ
/// mem::init 内の init_heap がこの LockedHeap を初期化する
#[global_allocator]
static KERNEL_ALLOCATOR: LockedHeap = LockedHeap::empty();

/// ELF エントリポイント
///
/// ブートローダーが構築した BootInfo の kernel_heap_addr フィールドを
/// 自分の KERNEL_ALLOCATOR のアドレスで上書きしてから kernel_entry を呼ぶ。
/// これにより mochios の init_heap が正しいアロケータを初期化できる。
#[no_mangle]
pub unsafe extern "sysv64" fn kernel_entry(boot_info_ptr: *mut mochios::BootInfo) -> ! {
    // kernel_heap_addr = &KERNEL_ALLOCATOR（init_heap がここを初期化する）
    (*boot_info_ptr).kernel_heap_addr =
        &KERNEL_ALLOCATOR as *const LockedHeap as u64;

    // ブートローダーがロードした initfs イメージを fs モジュールに設定
    mochios::init::fs::set_image(
        (*boot_info_ptr).initfs_addr,
        (*boot_info_ptr).initfs_size,
    );
    mochios::init::fs::set_rootfs(
        (*boot_info_ptr).rootfs_addr,
        (*boot_info_ptr).rootfs_size,
    );

    let boot_info: &'static mochios::BootInfo = &*(boot_info_ptr as *const _);
    mochios::kernel_entry(boot_info)
}
