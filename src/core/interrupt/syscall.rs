use core::arch::asm;
use crate::mem::gdt;

// MSR addresses
const IA32_STAR: u32 = 0xC000_0081;
const IA32_LSTAR: u32 = 0xC000_0082;
const IA32_FMASK: u32 = 0xC000_0084;

/// Write MSR with given index and value (rdx:rax)
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32 as u64;
    let high = (value >> 32) as u64;
    asm!(
        "mov ecx, {msr}",
        "mov eax, {low}",
        "mov edx, {high}",
        "wrmsr",
        msr = in(reg) msr,
        low = in(reg) (low as u32),
        high = in(reg) (high as u32),
        options(nostack, preserves_flags),
    );
}

/// Initialize syscall MSRs (STAR/LSTAR/FMASK)
pub fn init_syscall() {
    unsafe {
        let kernel_cs = gdt::kernel_code_selector() as u64;
        let user_cs = gdt::user_code_selector() as u64;
        // STAR: [63:48]=user_cs, [47:32]=kernel_cs
        let star = (user_cs << 48) | (kernel_cs << 32);
        wrmsr(IA32_STAR, star);

        // LSTAR: address of syscall entry point
        extern "C" {
            fn syscall_entry();
        }
        let addr = syscall_entry as usize as u64;
        wrmsr(IA32_LSTAR, addr);

        // FMASK: mask of flags to clear on syscall (clear interrupt flag)
        let fmask: u64 = (1 << 9); // clear IF
        wrmsr(IA32_FMASK, fmask);
    }
}

// Minimal syscall entry stub. This label is referenced by IA32_LSTAR.
// Implementation: swapgs -> call existing int80 handler via interrupt instruction
// (This is a pragmatic bridge until a full, register-preserving syscall path is implemented.)
core::arch::global_asm!(r#"
    .global syscall_entry
    .type syscall_entry, @function
syscall_entry:
    swapgs
    // invoke int 0x80 handler (existing path) and return using sysretq
    int $0x80
    swapgs
    sysretq
"#);
