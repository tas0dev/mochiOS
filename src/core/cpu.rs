//! CPU機能の初期化
//!
//! CR0/CR4レジスタの設定、SSE/FPUの有効化など

use crate::sprintln;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

static FSGSBASE_SUPPORTED: AtomicBool = AtomicBool::new(false);
static UMIP_SUPPORTED: AtomicBool = AtomicBool::new(false);
static SMEP_SUPPORTED: AtomicBool = AtomicBool::new(false);
static SMAP_SUPPORTED: AtomicBool = AtomicBool::new(false);
static SMAP_ENABLED: AtomicBool = AtomicBool::new(false);
static IBPB_SUPPORTED: AtomicBool = AtomicBool::new(false);
static CET_IBT_SUPPORTED: AtomicBool = AtomicBool::new(false);
static CET_SHSTK_SUPPORTED: AtomicBool = AtomicBool::new(false);
static RDRAND_SUPPORTED: AtomicBool = AtomicBool::new(false);
static SPEC_CTRL_MASK: AtomicU64 = AtomicU64::new(0);
static BOOT_ENTROPY: AtomicU64 = AtomicU64::new(0);
static CMOS_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, PartialEq, Eq)]
enum CpuVendor {
    Intel,
    Amd,
    Other,
}

#[derive(Clone, Copy)]
struct CpuidResult {
    eax: u32,
    ebx: u32,
    ecx: u32,
    edx: u32,
}

#[inline]
fn cpuid(leaf: u32, subleaf: u32) -> CpuidResult {
    // rbx は LLVM が予約するため、xchg で退避・復帰する。
    let eax: u32;
    let ebx: u64;
    let ecx: u32;
    let edx: u32;
    unsafe {
        asm!(
            "xchg {rbx_tmp}, rbx",
            "cpuid",
            "xchg {rbx_tmp}, rbx",
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            rbx_tmp = inout(reg) 0u64 => ebx,
            out("edx") edx,
            options(nomem, nostack)
        );
    }
    CpuidResult {
        eax,
        ebx: ebx as u32,
        ecx,
        edx,
    }
}

/// CPUの初期化（SSE/FPU/NXE/WP/UMIP/SMEP/SMAP/SpecCtrl有効化）
pub fn init() {
    crate::info!("Initializing CPU features...");

    detect_cpu_features();

    unsafe {
        enable_nxe();
        enable_write_protect();
        enable_fpu();
        enable_sse();
        enable_umip();
        enable_smep_smap();
        enable_speculation_controls();
        report_optional_control_flow_features();
    }
}

fn detect_cpu_features() {
    FSGSBASE_SUPPORTED.store(cpu_has_fsgsbase(), Ordering::Release);
    UMIP_SUPPORTED.store(cpu_has_umip(), Ordering::Release);
    SMEP_SUPPORTED.store(cpu_has_smep(), Ordering::Release);
    SMAP_SUPPORTED.store(cpu_has_smap(), Ordering::Release);
    RDRAND_SUPPORTED.store(cpu_has_rdrand(), Ordering::Release);
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

/// CR0.WP を有効化して supervisor write-protect を強制する
unsafe fn enable_write_protect() {
    let mut cr0 = read_cr0();
    const CR0_WP_BIT: u64 = 1 << 16;
    if (cr0 & CR0_WP_BIT) == 0 {
        cr0 |= CR0_WP_BIT;
        write_cr0(cr0);
        crate::info!("CR0.WP enabled");
    }
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
    if FSGSBASE_SUPPORTED.load(Ordering::Acquire) {
        cr4 |= 1 << 16;
        crate::info!("FSGSBASE enabled");
    } else {
        crate::info!("FSGSBASE not supported, using IA32_FS_BASE MSR");
    }

    // CR4レジスタに書き込み
    asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));
}

/// UMIP を有効化してユーザーモードからの SGDT/SIDT 等による情報漏えいを抑止する
unsafe fn enable_umip() {
    if !UMIP_SUPPORTED.load(Ordering::Acquire) {
        crate::warn!("UMIP not supported; skipping");
        return;
    }

    let mut cr4 = read_cr4();
    cr4 |= 1 << 11;
    write_cr4(cr4);
    crate::info!("UMIP enabled");
}

/// SMEP/SMAPを有効化
unsafe fn enable_smep_smap() {
    let mut cr4 = read_cr4();

    if SMEP_SUPPORTED.load(Ordering::Acquire) {
        // ビット20 (SMEP) をセット - カーネルモードでのユーザーページ実行禁止 (L-1修正)
        // ret2usr 等のカーネルモード特権昇格攻撃を防ぐ
        cr4 |= 1 << 20;
        crate::info!("SMEP enabled");
    } else {
        crate::warn!("SMEP not supported; skipping");
    }

    if SMAP_SUPPORTED.load(Ordering::Acquire) {
        // ビット21 (SMAP) をセット - カーネルモードでのユーザーページアクセス禁止 (L-1修正)
        // カーネルが誤ってユーザー空間メモリを読み書きする脆弱性を防ぐ
        cr4 |= 1 << 21;
        SMAP_ENABLED.store(true, Ordering::Release);
        crate::info!("SMAP enabled");
    } else {
        crate::warn!("SMAP not supported; skipping");
    }

    write_cr4(cr4);
}

unsafe fn enable_speculation_controls() {
    const IA32_SPEC_CTRL: u32 = 0x48;
    const IA32_PRED_CMD: u32 = 0x49;
    const IA32_EFER: u32 = 0xC000_0080;
    const SPEC_CTRL_IBRS: u64 = 1 << 0;
    const SPEC_CTRL_STIBP: u64 = 1 << 1;
    const SPEC_CTRL_BHI_DIS_S: u64 = 1 << 10;
    const EFER_AUTOIBRS: u64 = 1 << 21;

    let vendor = cpu_vendor();

    let mut spec_ctrl_mask = 0u64;
    if cpu_has_ibrs_ibpb() && !cpu_has_amd_autoibrs() {
        spec_ctrl_mask |= SPEC_CTRL_IBRS;
    }
    if cpu_has_stibp() {
        spec_ctrl_mask |= SPEC_CTRL_STIBP;
    }
    if vendor == CpuVendor::Intel && cpu_has_bhi_ctrl() {
        spec_ctrl_mask |= SPEC_CTRL_BHI_DIS_S;
    }
    if spec_ctrl_mask != 0 {
        let current = read_msr(IA32_SPEC_CTRL);
        write_msr(IA32_SPEC_CTRL, current | spec_ctrl_mask);
        SPEC_CTRL_MASK.store(spec_ctrl_mask, Ordering::Release);
        crate::info!("IA32_SPEC_CTRL hardened with mask {:#x}", spec_ctrl_mask);
    }

    if cpu_has_amd_autoibrs() {
        let efer = read_msr(IA32_EFER);
        if (efer & EFER_AUTOIBRS) == 0 {
            write_msr(IA32_EFER, efer | EFER_AUTOIBRS);
            crate::info!("AMD AutoIBRS enabled");
        }
    }

    if cpu_has_ibrs_ibpb() {
        let _ = IA32_PRED_CMD;
        IBPB_SUPPORTED.store(true, Ordering::Release);
        crate::info!("IBPB supported");
    }
}

unsafe fn report_optional_control_flow_features() {
    let ibt = cpu_has_cet_ibt();
    let shstk = cpu_has_cet_shadow_stack();
    CET_IBT_SUPPORTED.store(ibt, Ordering::Release);
    CET_SHSTK_SUPPORTED.store(shstk, Ordering::Release);
    crate::info!(
        "Optional CFI/CET support detected: IBT={}, shadow_stack={}",
        ibt,
        shstk
    );
}

fn cpuid_leaf7_ebx() -> u32 {
    cpuid(7, 0).ebx
}

fn cpuid_leaf7_ecx() -> u32 {
    cpuid(7, 0).ecx
}

fn cpuid_leaf7_edx() -> u32 {
    cpuid(7, 0).edx
}

fn cpu_has_cet_shadow_stack() -> bool {
    (cpuid_leaf7_ecx() & (1 << 7)) != 0
}

fn cpu_has_cet_ibt() -> bool {
    (cpuid_leaf7_edx() & (1 << 20)) != 0
}

fn cpuid_leaf7_subleaf2_edx() -> u32 {
    cpuid(7, 2).edx
}

fn cpuid_max_extended_leaf() -> u32 {
    cpuid(0x8000_0000, 0).eax
}

fn cpuid_ext_8000_0008_ebx() -> u32 {
    if cpuid_max_extended_leaf() < 0x8000_0008 {
        return 0;
    }
    cpuid(0x8000_0008, 0).ebx
}

fn cpuid_ext_8000_0021_eax() -> u32 {
    if cpuid_max_extended_leaf() < 0x8000_0021 {
        return 0;
    }
    cpuid(0x8000_0021, 0).eax
}

fn cpu_vendor() -> CpuVendor {
    let result = cpuid(0, 0);
    match (result.ebx, result.edx, result.ecx) {
        (0x756e_6547, 0x4965_6e69, 0x6c65_746e) => CpuVendor::Intel,
        (0x6874_7541, 0x6974_6e65, 0x444d_4163) => CpuVendor::Amd,
        _ => CpuVendor::Other,
    }
}

fn cpuid_leaf1_ecx() -> u32 {
    cpuid(1, 0).ecx
}

/// CPUID で UMIP サポートを確認 (leaf 7, ECX bit 2)
fn cpu_has_umip() -> bool {
    (cpuid_leaf7_ecx() & (1 << 2)) != 0
}

/// CPUID で FSGSBASE サポートを確認 (leaf 7, EBX bit 0)
fn cpu_has_fsgsbase() -> bool {
    (cpuid_leaf7_ebx() & (1 << 0)) != 0
}

/// CPUID で SMEP サポートを確認 (leaf 7, EBX bit 7)
fn cpu_has_smep() -> bool {
    (cpuid_leaf7_ebx() & (1 << 7)) != 0
}

/// CPUID で SMAP サポートを確認 (leaf 7, EBX bit 20)
fn cpu_has_smap() -> bool {
    (cpuid_leaf7_ebx() & (1 << 20)) != 0
}

/// CPUID で IBRS/IBPB サポートを確認 (leaf 7, EDX bit 26)
fn cpu_has_ibrs_ibpb() -> bool {
    (cpuid_leaf7_edx() & (1 << 26)) != 0
}

/// CPUID で STIBP サポートを確認
fn cpu_has_stibp() -> bool {
    ((cpuid_leaf7_edx() & (1 << 27)) != 0) || ((cpuid_ext_8000_0008_ebx() & (1 << 15)) != 0)
}

/// Intel BHI control bit の有無を確認 (leaf 7, subleaf 2, EDX bit 4)
fn cpu_has_bhi_ctrl() -> bool {
    (cpuid_leaf7_subleaf2_edx() & (1 << 4)) != 0
}

/// AMD AutoIBRS サポートを確認 (Fn8000_0021:EAX bit 8)
fn cpu_has_amd_autoibrs() -> bool {
    cpu_vendor() == CpuVendor::Amd && (cpuid_ext_8000_0021_eax() & (1 << 8)) != 0
}

/// CPUID で RDRAND サポートを確認 (leaf 1, ECX bit 30)
fn cpu_has_rdrand() -> bool {
    (cpuid_leaf1_ecx() & (1 << 30)) != 0
}

/// 可能なら CPU のハードウェア乱数 (RDRAND) を返す
pub fn hw_random_u64() -> Option<u64> {
    if !RDRAND_SUPPORTED.load(Ordering::Acquire) {
        return None;
    }
    for _ in 0..10 {
        let value: u64;
        let ok: u8;
        unsafe {
            asm!(
                "rdrand {val}",
                "setc {ok}",
                val = out(reg) value,
                ok = out(reg_byte) ok,
                options(nomem, nostack)
            );
        }
        if ok != 0 {
            return Some(value);
        }
    }
    None
}

/// FS ベースを書き込む (WRFSBASE または IA32_FS_BASE MSR)
///
/// # Safety
/// 呼び出し側は `val` が現在スレッドの有効な TLS ベース値であることを保証する必要がある。
pub unsafe fn write_fs_base(val: u64) {
    if FSGSBASE_SUPPORTED.load(Ordering::Acquire) {
        asm!("wrfsbase {}", in(reg) val, options(nostack, preserves_flags));
    } else {
        // IA32_FS_BASE MSR = 0xC0000100
        let lo = val as u32;
        let hi = (val >> 32) as u32;
        asm!("wrmsr", in("ecx") 0xC000_0100u32, in("eax") lo, in("edx") hi, options(nomem, nostack));
    }
}

/// FS ベースを読み込む (RDFSBASE または IA32_FS_BASE MSR)
///
/// # Safety
/// 呼び出し側は、現在の実行コンテキストで FS ベース読み出しが安全であることを保証する必要がある。
pub unsafe fn read_fs_base() -> u64 {
    if FSGSBASE_SUPPORTED.load(Ordering::Acquire) {
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

pub fn is_smap_enabled() -> bool {
    SMAP_ENABLED.load(Ordering::Acquire)
}

#[inline]
pub fn speculation_barrier() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        asm!("lfence", options(nomem, nostack, preserves_flags));
    }
}

pub fn branch_predictor_barrier() {
    const IA32_PRED_CMD: u32 = 0x49;
    if IBPB_SUPPORTED.load(Ordering::Acquire) {
        unsafe {
            write_msr(IA32_PRED_CMD, 1);
        }
    }
    speculation_barrier();
}

pub fn reassert_runtime_hardening() {
    unsafe {
        let mut cr0 = read_cr0();
        let mut cr4 = read_cr4();
        cr0 |= 1 << 16;
        if UMIP_SUPPORTED.load(Ordering::Acquire) {
            cr4 |= 1 << 11;
        }
        if SMEP_SUPPORTED.load(Ordering::Acquire) {
            cr4 |= 1 << 20;
        }
        if SMAP_SUPPORTED.load(Ordering::Acquire) {
            cr4 |= 1 << 21;
        }
        write_cr0(cr0);
        write_cr4(cr4);
        let spec_ctrl_mask = SPEC_CTRL_MASK.load(Ordering::Acquire);
        if spec_ctrl_mask != 0 {
            const IA32_SPEC_CTRL: u32 = 0x48;
            let current = read_msr(IA32_SPEC_CTRL);
            if (current & spec_ctrl_mask) != spec_ctrl_mask {
                write_msr(IA32_SPEC_CTRL, current | spec_ctrl_mask);
            }
        }
    }
}

#[inline]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | (lo as u64)
}

#[inline]
fn aslr_mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[inline]
fn cmos_read(reg: u8) -> u8 {
    use x86_64::instructions::port::Port;
    unsafe {
        let mut index = Port::<u8>::new(0x70);
        let mut data = Port::<u8>::new(0x71);
        index.write(0x80 | reg);
        data.read()
    }
}

#[inline]
fn bcd_to_bin(v: u8) -> u8 {
    (v & 0x0f) + ((v >> 4) * 10)
}

fn rtc_entropy_u64() -> u64 {
    let _guard = CMOS_LOCK.lock();
    while (cmos_read(0x0A) & 0x80) != 0 {
        core::hint::spin_loop();
    }
    let mut sec = cmos_read(0x00);
    let mut min = cmos_read(0x02);
    let mut hour = cmos_read(0x04);
    let mut day = cmos_read(0x07);
    let mut mon = cmos_read(0x08);
    let mut year = cmos_read(0x09);
    let reg_b = cmos_read(0x0B);
    if (reg_b & 0x04) == 0 {
        sec = bcd_to_bin(sec);
        min = bcd_to_bin(min);
        hour = bcd_to_bin(hour & 0x7f);
        day = bcd_to_bin(day);
        mon = bcd_to_bin(mon);
        year = bcd_to_bin(year);
    }
    (sec as u64)
        | ((min as u64) << 8)
        | ((hour as u64) << 16)
        | ((day as u64) << 24)
        | ((mon as u64) << 32)
        | ((year as u64) << 40)
}

/// ASLR 用のブート時エントロピーを返す（同一ブート中は固定、ブート間は変化を期待）。
pub fn boot_entropy_u64() -> u64 {
    let cached = BOOT_ENTROPY.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }

    let mut seed = rdtsc()
        ^ rtc_entropy_u64().rotate_left(19)
        ^ (core::ptr::addr_of!(BOOT_ENTROPY) as u64).rotate_left(7);
    if let Some(hw) = hw_random_u64() {
        seed ^= hw.rotate_left(23);
    }
    if seed == 0 {
        seed = 0x243f_6a88_85a3_08d3;
    }

    let mixed = aslr_mix64(seed);
    match BOOT_ENTROPY.compare_exchange(0, mixed, Ordering::SeqCst, Ordering::Relaxed) {
        Ok(_) => mixed,
        Err(v) => v,
    }
}

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    ((hi as u64) << 32) | (lo as u64)
}

unsafe fn write_msr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack)
    );
}

#[inline]
unsafe fn read_cr0() -> u64 {
    let cr0: u64;
    asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
    cr0
}

#[inline]
unsafe fn write_cr0(val: u64) {
    asm!("mov cr0, {}", in(reg) val, options(nomem, nostack));
}

#[inline]
unsafe fn read_cr4() -> u64 {
    let cr4: u64;
    asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
    cr4
}

#[inline]
unsafe fn write_cr4(val: u64) {
    asm!("mov cr4, {}", in(reg) val, options(nomem, nostack));
}
