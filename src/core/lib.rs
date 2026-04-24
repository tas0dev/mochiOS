#![no_std]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![allow(unused)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]

#[cfg(feature = "kcfi")]
compile_error!(
    "feature `kcfi` is intentionally gated off: the current mochiOS build does not have a \
     verified Rust/LLVM KCFI pipeline for this freestanding x86_64 kernel. Leaving it \
     selectable without end-to-end verification would be unsound."
);

#[cfg(feature = "cet-ibt")]
compile_error!(
    "feature `cet-ibt` is intentionally gated off: hand-written syscall/interrupt/trampoline \
     assembly has not yet been fully annotated and inspected for ENDBR64 compliance."
);

#[cfg(feature = "cet-shadow-stack")]
compile_error!(
    "feature `cet-shadow-stack` is intentionally gated off: kernel shadow-stack allocation, \
     context-switch save/restore, and signal integration are not yet complete."
);

extern crate alloc;

/// エラー型定義
pub mod result;

/// 監査ログ
pub mod audit;

/// 割込み管理
pub mod interrupt;

/// カーネル本体
pub mod kernel;
pub mod kmod;

/// メモリ管理、GDT、TSSを含む
pub mod mem;

/// ELF周り
pub mod elf;

/// パニックハンドラ
pub mod panic;

/// タスク管理
pub mod task;

/// システムコール
pub mod syscall;

/// 起動時初期化
pub mod init;

/// ユーティリティモジュール
pub mod util;

/// CPU機能の初期化
pub mod cpu;
/// per-CPU状態管理
pub mod percpu;

pub use kernel::kernel_entry;
pub use result::{Kernel, Result};

/// デバイス情報
#[repr(C)]
pub struct BootInfo {
    /// 物理メモリオフセット
    pub physical_memory_offset: u64,
    /// フレームバッファアドレス
    pub framebuffer_addr: u64,
    /// フレームバッファサイズ
    pub framebuffer_size: usize,
    /// 画面の幅（ピクセル）
    pub screen_width: usize,
    /// 画面の高さ（ピクセル）
    pub screen_height: usize,
    /// 1行あたりのバイト数
    pub stride: usize,
    /// メモリマップのアドレス
    pub memory_map_addr: u64,
    /// メモリマップのエントリ数
    pub memory_map_len: usize,
    /// メモリマップの各エントリサイズ
    pub memory_map_entry_size: usize,
    /// カーネルアロケータの制御構造体へのアドレス（kernel binaryが起動時に設定）
    pub kernel_heap_addr: u64,
    /// initfs イメージの物理アドレス（ブートローダーが設定）
    pub initfs_addr: u64,
    /// initfs イメージのサイズ（バイト）
    pub initfs_size: usize,
    /// rootfs (ext2) イメージの物理アドレス（通常は0。必要なら別経路で設定）
    pub rootfs_addr: u64,
    /// rootfs イメージのサイズ（バイト。通常は0）
    pub rootfs_size: usize,
}

/// メモリ領域の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub enum MemoryType {
    /// 使用可能
    Usable,
    /// 予約済み
    Reserved,
    /// ACPIで再利用可能
    AcpiReclaimable,
    /// ACPI NVS
    AcpiNvs,
    /// 不良メモリ
    BadMemory,
    /// ブートローダーで使用中
    BootloaderReclaimable,
    /// カーネルスタック
    KernelStack,
    /// ページテーブル
    PageTable,
    /// フレームバッファ
    Framebuffer,
}

/// メモリマップエントリ
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct MemoryRegion {
    /// 開始アドレス
    pub start: u64,
    /// 長さ（バイト）
    pub len: u64,
    /// 領域の種類
    pub region_type: MemoryType,
}
