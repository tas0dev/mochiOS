//! TSS管理モジュール
//!
//! TSSを管理

use crate::{info, sprintln};
use spin::Once;
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// ダブルフォルト用ISTインデックス
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

static TSS: Once<TaskStateSegment> = Once::new();

/// TSSを初期化して返す
///
/// ## Returns
/// - 初期化されたTSSへの参照
#[allow(unused_unsafe)]
pub fn init() -> &'static TaskStateSegment {
    info!("Initializing TSS...");

    TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();

        // ダブルフォルト用の専用スタックを設定
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 4096 * 16; // 64KB (増量: 20KB→64KB)
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

            let stack_start = VirtAddr::from_ptr(unsafe { &raw const STACK });
            let stack_end = stack_start + STACK_SIZE as u64;
            info!(
                "  IST[{}] stack: {:#x}",
                DOUBLE_FAULT_IST_INDEX,
                stack_end.as_u64()
            );
            stack_end
        };

        // ユーザーモードからカーネルモードへの遷移用のRing0スタックを設定
        tss.privilege_stack_table[0] = {
            const RING0_STACK_SIZE: usize = 4096 * 32; // 128KB (増量: 16KB→128KB、fs.service大容量バッファ対応)
            static mut RING0_STACK: [u8; RING0_STACK_SIZE] = [0; RING0_STACK_SIZE];

            let stack_start = VirtAddr::from_ptr(unsafe { &raw const RING0_STACK });
            let stack_end = stack_start + RING0_STACK_SIZE as u64;
            info!("  Ring0 stack (RSP0): {:#x}", stack_end.as_u64());
            stack_end
        };

        info!("TSS configured:");
        info!(
            "  IST[0] stack: {:#x}",
            tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize].as_u64()
        );
        info!(
            "  Ring0 stack (RSP0): {:#x}",
            tss.privilege_stack_table[0].as_u64()
        );
        tss
    })
}

/// Ring 0スタック (RSP0) を更新
///
/// コンテキストスイッチ時に呼び出し、次のスレッドのカーネルスタックを設定する
///
/// ## Arguments
/// - `rsp`: 新しいRSP0の値 (次のスレッドのカーネルスタックのアドレス)
pub fn set_rsp0(rsp: u64) {
    if let Some(tss) = TSS.get() {
        // TSSは参照として取得されるが、RSP0は実行時に変更する必要があるため、
        // 内部可変性を持つか、ポインタ経由で変更する
        let ptr = tss as *const TaskStateSegment as *mut TaskStateSegment;
        unsafe {
            // RSP0更新中の割り込み/コンテキストスイッチを防ぐため、
            // 割り込みを一時的に無効化してアトミックに更新
            x86_64::instructions::interrupts::without_interrupts(|| {
                (*ptr).privilege_stack_table[0] = VirtAddr::new(rsp);
            });
        }
    }
}
