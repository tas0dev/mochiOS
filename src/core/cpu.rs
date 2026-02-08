//! CPU機能の初期化
//!
//! CR0/CR4レジスタの設定、SSE/FPUの有効化など

use core::arch::asm;
use crate::sprintln;

/// CPUの初期化（SSE/FPU有効化）
pub fn init() {
    crate::info!("Initializing CPU features...");
    
    unsafe {
        enable_fpu();
        enable_sse();
    }
}

/// FPUを有効化
unsafe fn enable_fpu() {
    // CR0レジスタを読み取り
    let mut cr0: u64;
    asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
    
    // ビット2 (EM - Emulation) をクリア
    cr0 &= !(1 << 2);
    // ビット1 (MP - Monitor Coprocessor) をセット
    cr0 |= 1 << 1;
    // ビット5 (NE - Numeric Error) をセット
    cr0 |= 1 << 5;
    
    // CR0レジスタに書き込み
    asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack));
}

/// SSEを有効化
unsafe fn enable_sse() {
    // CR4レジスタを読み取り
    let mut cr4: u64;
    asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
    
    // ビット9 (OSFXSR) をセット - FXSAVE/FXRSTOR命令のサポート
    cr4 |= 1 << 9;
    // ビット10 (OSXMMEXCPT) をセット - SSE例外のサポート
    cr4 |= 1 << 10;
    
    // CR4レジスタに書き込み
    asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));
}
