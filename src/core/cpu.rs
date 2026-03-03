//! CPU機能の初期化
//!
//! CR0/CR4レジスタの設定、SSE/FPUの有効化など

use crate::sprintln;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

static FSGSBASE_SUPPORTED: AtomicBool = AtomicBool::new(false);

/// CPUの初期化（SSE/FPU有効化、NXE有効化）
pub fn init() {
    crate::info!("Initializing CPU features...");

    unsafe {
        enable_nxe();
        enable_fpu();
        enable_sse();
    }
}

/// EFER.NXEを有効化（NO_EXECUTEページテーブルフラグを機能させる）
///
/// NXE (No-Execute Enable) を IA32_EFER MSR (0xC0000080) のビット11にセットする。
/// これにより PTE の bit 63 (NO_EXECUTE) が有効になり、データページでのコード実行を防ぐ。
unsafe fn enable_nxe() {
    const IA32_EFER: u32 = 0xC000_0080;
    const NXE_BIT: u64 = 1 << 11;
    let lo: u32;
    let hi: u32;
    asm!("rdmsr", in("ecx") IA32_EFER, out("eax") lo, out("edx") hi, options(nomem, nostack));
    let efer = ((hi as u64) << 32) | (lo as u64);
    if efer & NXE_BIT == 0 {
        let new_efer = efer | NXE_BIT;
        asm!(
            "wrmsr",
            in("ecx") IA32_EFER,
            in("eax") (new_efer as u32),
            in("edx") ((new_efer >> 32) as u32),
            options(nomem, nostack)
        );
        crate::info!("EFER.NXE enabled");
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
    // ビット16 (FSGSBASE) をセット - RDFSBASE/WRFSBASE命令のサポート (TLS用)
    // CPUID leaf 7, EBX bit 0 でサポート確認
    if cpu_has_fsgsbase() {
        cr4 |= 1 << 16;
        FSGSBASE_SUPPORTED.store(true, Ordering::Relaxed);
        crate::info!("FSGSBASE enabled");
    } else {
        crate::info!("FSGSBASE not supported, using IA32_FS_BASE MSR");
    }

    // ビット20 (SMEP) をセット - カーネルモードでのユーザーページ実行禁止 (L-1修正)
    // ret2usr 等のカーネルモード特権昇格攻撃を防ぐ
    cr4 |= 1 << 20;

    // ビット21 (SMAP) をセット - カーネルモードでのユーザーページアクセス禁止 (L-1修正)
    // カーネルが誤ってユーザー空間メモリを読み書きする脆弱性を防ぐ
    cr4 |= 1 << 21;

    // CR4レジスタに書き込み
    asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));
}

/// CPUID で FSGSBASE サポートを確認 (leaf 7, EBX bit 0)
fn cpu_has_fsgsbase() -> bool {
    // rbx は LLVM が予約するため xchg で保存/復元する
    let ebx: u64;
    unsafe {
        asm!(
            "xchg {tmp}, rbx",
            "cpuid",
            "xchg {tmp}, rbx",
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            tmp = inout(reg) 0u64 => ebx,
            out("edx") _,
            options(nomem, nostack)
        );
    }
    (ebx as u32 & 1) != 0
}

/// FS ベースを書き込む (WRFSBASE または IA32_FS_BASE MSR)
pub unsafe fn write_fs_base(val: u64) {
    if FSGSBASE_SUPPORTED.load(Ordering::Relaxed) {
        asm!("wrfsbase {}", in(reg) val, options(nostack, preserves_flags));
    } else {
        // IA32_FS_BASE MSR = 0xC0000100
        let lo = val as u32;
        let hi = (val >> 32) as u32;
        asm!("wrmsr", in("ecx") 0xC000_0100u32, in("eax") lo, in("edx") hi, options(nomem, nostack));
    }
}

/// FS ベースを読み込む (RDFSBASE または IA32_FS_BASE MSR)
pub unsafe fn read_fs_base() -> u64 {
    if FSGSBASE_SUPPORTED.load(Ordering::Relaxed) {
        let val: u64;
        asm!("rdfsbase {}", out(reg) val, options(nostack, preserves_flags));
        val
    } else {
        let lo: u32;
        let hi: u32;
        asm!("rdmsr", in("ecx") 0xC000_0100u32, out("eax") lo, out("edx") hi, options(nomem, nostack));
        ((hi as u64) << 32) | (lo as u64)
    }
}
