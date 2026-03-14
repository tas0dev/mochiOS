//! per-CPU 状態管理（SMP拡張の基盤）
//!
//! APIC ID をキーに CPU ローカルスロットを選択する。

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::control::Cr3;

const MAX_CPUS: usize = 64;
const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;
pub const GS_SYSCALL_KERNEL_RSP_OFFSET: usize = 8;
pub const GS_SYSCALL_USER_RSP_TMP_OFFSET: usize = 24;

#[repr(C)]
struct PerCpuState {
    kernel_cr3: AtomicU64,
    syscall_kernel_rsp: AtomicU64,
    current_thread_id: AtomicU64,
    syscall_user_rsp_tmp: AtomicU64,
}

impl PerCpuState {
    const fn new() -> Self {
        Self {
            kernel_cr3: AtomicU64::new(0),
            syscall_kernel_rsp: AtomicU64::new(0),
            current_thread_id: AtomicU64::new(0),
            syscall_user_rsp_tmp: AtomicU64::new(0),
        }
    }
}

static CPU_STATES: [PerCpuState; MAX_CPUS] = [const { PerCpuState::new() }; MAX_CPUS];

#[inline]
fn state_for_current_cpu() -> &'static PerCpuState {
    &CPU_STATES[current_cpu_id()]
}

#[inline(never)]
fn halt_unsupported_cpu(_apic_id: u32) -> ! {
    x86_64::instructions::interrupts::disable();
    loop {
        x86_64::instructions::hlt();
    }
}

#[inline]
unsafe fn write_kernel_gs_base(base: u64) {
    let lo = base as u32;
    let hi = (base >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") IA32_KERNEL_GS_BASE,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack)
    );
}

#[inline]
pub fn current_cpu_id() -> usize {
    let apic_id = local_apic_id() as usize;
    if apic_id < MAX_CPUS {
        apic_id
    } else {
        halt_unsupported_cpu(apic_id as u32)
    }
}

#[inline]
fn local_apic_id() -> u32 {
    // CPUID leaf 1: EBX[31:24] = Initial APIC ID
    let ebx: u64;
    unsafe {
        asm!(
            "xchg {tmp}, rbx",
            "cpuid",
            "xchg {tmp}, rbx",
            inout("eax") 1u32 => _,
            in("ecx") 0u32,
            tmp = inout(reg) 0u64 => ebx,
            out("edx") _,
            options(nomem, nostack)
        );
    }
    ((ebx as u32) >> 24) & 0xff
}

pub fn init_boot_cpu(syscall_kernel_rsp: u64) {
    let apic_id = local_apic_id() as usize;
    assert!(
        apic_id < MAX_CPUS,
        "Boot CPU APIC ID {} exceeds MAX_CPUS {}",
        apic_id,
        MAX_CPUS
    );

    let (cr3, _) = Cr3::read();
    let state = &CPU_STATES[apic_id];
    state
        .kernel_cr3
        .store(cr3.start_address().as_u64(), Ordering::SeqCst);
    state
        .syscall_kernel_rsp
        .store(syscall_kernel_rsp, Ordering::SeqCst);
    state.current_thread_id.store(0, Ordering::SeqCst);
    state.syscall_user_rsp_tmp.store(0, Ordering::SeqCst);
    install_current_cpu_gs_base();
}

pub fn install_current_cpu_gs_base() {
    let state = state_for_current_cpu() as *const PerCpuState as u64;
    unsafe {
        write_kernel_gs_base(state);
    }
}

pub fn kernel_cr3() -> u64 {
    state_for_current_cpu().kernel_cr3.load(Ordering::SeqCst)
}

pub fn set_syscall_kernel_rsp(rsp: u64) {
    state_for_current_cpu()
        .syscall_kernel_rsp
        .store(rsp, Ordering::SeqCst);
}

pub fn syscall_kernel_rsp() -> u64 {
    state_for_current_cpu()
        .syscall_kernel_rsp
        .load(Ordering::SeqCst)
}

pub fn current_thread_raw_id() -> u64 {
    state_for_current_cpu()
        .current_thread_id
        .load(Ordering::SeqCst)
}

pub fn set_current_thread_raw_id(id: u64) {
    state_for_current_cpu()
        .current_thread_id
        .store(id, Ordering::SeqCst);
}
