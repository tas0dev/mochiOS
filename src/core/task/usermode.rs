//! ユーザーモード実行サポート

use core::arch::asm;
use crate::mem::gdt;

/// ユーザーモードでコードを実行する
///
/// # 引数
/// - `entry`: ユーザーモードで実行する関数のアドレス
/// - `user_stack`: ユーザースタックのトップアドレス
///
/// # 注意
/// この関数は戻らない
pub unsafe fn jump_to_usermode(entry: u64, user_stack: u64) -> ! {
    let user_cs = gdt::user_code_selector().0 as u64 | 3; // RPL=3
    let user_ss = gdt::user_data_selector().0 as u64 | 3; // RPL=3

    // GDTエントリの内容を読み取って確認
    let cs_selector = gdt::user_code_selector().0;
    let ss_selector = gdt::user_data_selector().0;

    let gdtr = read_gdtr();
    let gdt_base = gdtr.0;

    // CSのGDTエントリを読み取る
    let cs_index = (cs_selector >> 3) as usize;
    let cs_entry_ptr = (gdt_base + (cs_index * 8) as u64) as *const u64;
    let cs_entry = core::ptr::read_volatile(cs_entry_ptr);
    let cs_dpl = (cs_entry >> 45) & 0b11;

    // SSのGDTエントリを読み取る
    let ss_index = (ss_selector >> 3) as usize;
    let ss_entry_ptr = (gdt_base + (ss_index * 8) as u64) as *const u64;
    let ss_entry = core::ptr::read_volatile(ss_entry_ptr);
    let ss_dpl = (ss_entry >> 45) & 0b11;

    crate::debug!("GDT Check:");
    crate::debug!("  CS selector={:#x}, index={}, entry={:#018x}, DPL={}",
                  cs_selector, cs_index, cs_entry, cs_dpl);
    crate::debug!("  SS selector={:#x}, index={}, entry={:#018x}, DPL={}",
                  ss_selector, ss_index, ss_entry, ss_dpl);
    crate::debug!("  Final CS={:#x} (with RPL=3), SS={:#x} (with RPL=3)",
                  user_cs, user_ss);

    crate::debug!("Jumping to usermode: entry={:#x}, stack={:#x}",
                  entry, user_stack);

    // iretqスタックフレームを構築:
    // SS, RSP, RFLAGS, CS, RIP
    asm!(
        "cli",

        // データセグメントをユーザーセグメントに設定（iretq前）
        "mov ax, {ss:x}",
        "mov ds, ax",
        "mov es, ax",

        // iretq用のスタックフレームをプッシュ
        "push {ss}",       // SS (ユーザーデータセグメント)
        "push {rsp}",      // RSP (ユーザースタック)
        "pushfq",          // 現在のRFLAGSを保存
        "pop r11",
        "or r11, 0x200",   // IF (Interrupt Flag) を設定
        "push r11",        // RFLAGS
        "push {cs}",       // CS (ユーザーコードセグメント)
        "push {rip}",      // RIP (エントリーポイント)

        // iretqでユーザーモードへジャンプ
        "iretq",

        ss = in(reg) user_ss,
        rsp = in(reg) user_stack,
        cs = in(reg) user_cs,
        rip = in(reg) entry,
        options(noreturn)
    );
}

/// GDTRを読み取る
fn read_gdtr() -> (u64, u16) {
    let mut gdtr: [u8; 10] = [0; 10];
    unsafe {
        asm!("sgdt [{}]", in(reg) gdtr.as_mut_ptr(), options(nostack));
    }
    let limit = u16::from_le_bytes([gdtr[0], gdtr[1]]);
    let base = u64::from_le_bytes([
        gdtr[2], gdtr[3], gdtr[4], gdtr[5],
        gdtr[6], gdtr[7], gdtr[8], gdtr[9],
    ]);
    (base, limit)
}

