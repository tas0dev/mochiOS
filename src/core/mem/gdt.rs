//! GDT管理モジュール
//!
//! Global Descriptor Tableを管理

use crate::mem::tss;
use crate::sprintln;
use core::arch::asm;
use spin::Once;
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};

/// ダブルフォルト用ISTインデックス（TSSと同じ値を使用）
pub const DOUBLE_FAULT_IST_INDEX: u16 = tss::DOUBLE_FAULT_IST_INDEX;

static GDT: Once<(GlobalDescriptorTable, Selectors)> = Once::new();

/// GDTセレクタ
#[allow(unused)]
struct Selectors {
    code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
    user_code_selector: SegmentSelector,
    user_data_selector: SegmentSelector,
}

/// GDTを初期化
pub fn init() {
    sprintln!("Initializing GDT...");

    // TSSを初期化
    let tss = tss::init();

    // GDTを初期化
    let (gdt, selectors) = GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        let data_selector = gdt.append(Descriptor::kernel_data_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(tss));
        // user segments (RPL=3)
        let user_code_selector = gdt.append(Descriptor::user_code_segment());
        let user_data_selector = gdt.append(Descriptor::user_data_segment());

        sprintln!("GDT entries created:");
        sprintln!("  Code selector: {:?}", code_selector);
        sprintln!("  Data selector: {:?}", data_selector);
        sprintln!("  TSS selector: {:?}", tss_selector);

        (
            gdt,
            Selectors {
                code_selector,
                data_selector,
                tss_selector,
                user_code_selector,
                user_data_selector,
            },
        )
    });

    unsafe {
        // GDTをロード
        gdt.load();

        // Boot Services終了後はカーネルのセグメントに切り替え
        set_cs(selectors.code_selector);
        set_data_segments(selectors.data_selector);

        // TSSをロード
        load_tss(selectors.tss_selector);
    }

    sprintln!("GDT loaded with TSS");
}

/// ユーザーモード用コードセレクタ（RPL=3）を返す
pub fn user_code_selector() -> u16 {
    GDT.get()
        .expect("GDT not initialized")
        .1
        .user_code_selector
        .0
}

/// ユーザーモード用データセレクタ（RPL=3）を返す
pub fn user_data_selector() -> u16 {
    GDT.get()
        .expect("GDT not initialized")
        .1
        .user_data_selector
        .0
}

#[allow(unused)]
/// データセグメントレジスタを設定
unsafe fn set_data_segments(selector: SegmentSelector) {
    asm!(
        "mov ds, {0:x}",
        "mov es, {0:x}",
        "mov fs, {0:x}",
        "mov gs, {0:x}",
        "mov ss, {0:x}",
        in(reg) selector.0,
        options(nostack, preserves_flags)
    );
}

#[allow(unused)]
/// コードセグメントを設定（far returnを使用）
unsafe fn set_cs(selector: SegmentSelector) {
    asm!(
        "push {sel}",
        "lea {tmp}, [rip + 2f]",
        "push {tmp}",
        "retfq",
        "2:",
        sel = in(reg) u64::from(selector.0),
        tmp = lateout(reg) _,
        options(preserves_flags)
    );
}
