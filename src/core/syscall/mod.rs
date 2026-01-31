//! システムコール

pub mod ipc;
pub mod task;
pub mod time;
pub mod exec;

mod types;

pub use types::{SyscallNumber, EAGAIN, EINVAL, ENOSYS};

use core::arch::asm;
use x86_64::structures::idt::InterruptStackFrame;

/// システムコールのディスパッチ
pub fn dispatch(num: u64, arg0: u64, arg1: u64, _arg2: u64, _arg3: u64, _arg4: u64) -> u64 {
    match num {
        x if x == SyscallNumber::Yield as u64 => task::yield_now(),
        x if x == SyscallNumber::GetTicks as u64 => time::get_ticks(),
        x if x == SyscallNumber::IpcSend as u64 => ipc::send(arg0, arg1),
        x if x == SyscallNumber::IpcRecv as u64 => ipc::recv(arg0),
        x if x == SyscallNumber::Exec as u64 => exec::exec_kernel(arg0),
        _ => ENOSYS,
    }
}

/// システムコール割り込みハンドラ (int 0x80)
pub extern "x86-interrupt" fn syscall_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let num: u64;
    let arg0: u64;
    let arg1: u64;
    let arg2: u64;
    let arg3: u64;
    let arg4: u64;

    unsafe {
        asm!(
            "mov {0}, rax",
            "mov {1}, rdi",
            "mov {2}, rsi",
            "mov {3}, rdx",
            "mov {4}, r10",
            "mov {5}, r8",
            out(reg) num,
            out(reg) arg0,
            out(reg) arg1,
            out(reg) arg2,
            out(reg) arg3,
            out(reg) arg4,
            options(nomem, nostack, preserves_flags)
        );
    }

    let ret = dispatch(num, arg0, arg1, arg2, arg3, arg4);

    unsafe {
        asm!(
            "mov rax, {0}",
            in(reg) ret,
            options(nomem, nostack, preserves_flags)
        );
    }
}
