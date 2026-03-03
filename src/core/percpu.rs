//! per-CPU 状態管理（SMP拡張の基盤）
//!
//! APIC ID をキーに CPU ローカルスロットを選択する。

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::control::Cr3;

const MAX_CPUS: usize = 64;

#[repr(C)]
struct PerCpuState {
    kernel_cr3: AtomicU64,
    syscall_kernel_rsp: AtomicU64,
}

impl PerCpuState {
    const fn new() -> Self {
        Self {
            kernel_cr3: AtomicU64::new(0),
            syscall_kernel_rsp: AtomicU64::new(0),
        }
    }
}

static CPU_STATES: [PerCpuState; MAX_CPUS] = [const { PerCpuState::new() }; MAX_CPUS];

#[inline]
fn state_for_current_cpu() -> &'static PerCpuState {
    &CPU_STATES[current_cpu_id()]
}

#[inline]
pub fn current_cpu_id() -> usize {
    (local_apic_id() as usize) % MAX_CPUS
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
    let (cr3, _) = Cr3::read();
    let state = state_for_current_cpu();
    state
        .kernel_cr3
        .store(cr3.start_address().as_u64(), Ordering::SeqCst);
    state
        .syscall_kernel_rsp
        .store(syscall_kernel_rsp, Ordering::SeqCst);
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
