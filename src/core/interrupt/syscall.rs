use crate::mem::gdt;
use core::arch::asm;

// MSR addresses
const IA32_STAR: u32 = 0xC000_0081;
const IA32_LSTAR: u32 = 0xC000_0082;
const IA32_FMASK: u32 = 0xC000_0084;

/// MSRに値を書き込む
///
/// ## Arguments
/// - `msr`: MSRのアドレス
/// - `value`: 書き込む値
///
/// ## Safety
/// - `msr`は有効なMSRアドレスでなければならない
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nostack, preserves_flags),
    );
}

/// システムコールのMSRを初期化する
pub fn init_syscall() {
    unsafe {
        let kernel_cs = gdt::kernel_code_selector() as u64;
        let user_cs = gdt::user_code_selector() as u64;
        // STAR: [63:48]=user_cs, [47:32]=kernel_cs
        let star = (user_cs << 48) | (kernel_cs << 32);
        wrmsr(IA32_STAR, star);

        // LSTAR: システムコールエントリポイントのアドレス
        extern "C" {
            fn syscall_entry();
        }
        let addr = syscall_entry as *const () as usize as u64;
        wrmsr(IA32_LSTAR, addr);

        // FMASK: システムコール実行時にクリアするフラグマスク。ここではIFをクリアして割り込みを禁止する。
        let fmask: u64 = (1 << 9); // clear IF
        wrmsr(IA32_FMASK, fmask);
    }
}

// Note: syscall_entryはsyscall/syscall_entry.rsで実際のハンドラとして定義されている。
