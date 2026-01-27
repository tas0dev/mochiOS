//! システムコール

pub mod ipc;
pub mod task;
pub mod time;
pub mod console;
pub mod fs;
pub mod keyboard;
pub mod linux;

mod types;

pub use types::{SyscallNumber, EAGAIN, EINVAL, ENOSYS, ENOENT, ENODATA};

use core::arch::asm;
use linux as linux_sys;
use x86_64::structures::idt::InterruptStackFrame;

/// システムコールのディスパッチ
pub fn dispatch(num: u64, arg0: u64, arg1: u64, _arg2: u64, _arg3: u64, _arg4: u64) -> u64 {
	match num {
		x if x == SyscallNumber::Yield as u64 => task::yield_now(),
		x if x == SyscallNumber::GetTicks as u64 => time::get_ticks(),
		x if x == SyscallNumber::IpcSend as u64 => ipc::send(arg0, arg1),
		x if x == SyscallNumber::IpcRecv as u64 => ipc::recv(arg0),
		x if x == SyscallNumber::ConsoleWrite as u64 => console::write(arg0, arg1),
		x if x == SyscallNumber::InitfsRead as u64 => fs::read(arg0, arg1, _arg2, _arg3),
		x if x == SyscallNumber::Exit as u64 => task::exit(arg0),
		x if x == SyscallNumber::KeyboardRead as u64 => keyboard::read_char(),
		x if x == SyscallNumber::GetThreadId as u64 => task::get_thread_id(),
		x if x == SyscallNumber::GetThreadIdByName as u64 => task::get_thread_id_by_name(arg0, arg1),
		_ => {
			match num {
				x if x == linux_sys::SYS_READ => { // read(fd, buf, count)
					let fd = arg0; let buf = arg1; let count = _arg2;
					return linux_read(fd, buf, count);
				}
				x if x == linux_sys::SYS_WRITE => { // write(fd, buf, count)
					let fd = arg0; let buf = arg1; let count = _arg2;
					return linux_write(fd, buf, count);
				}
				x if x == linux_sys::SYS_MMAP => { // mmap
					return ENOSYS;
				}
				x if x == linux_sys::SYS_BRK => { // brk
					return ENOSYS;
				}
				x if x == linux_sys::SYS_EXIT => { // exit
					let code = arg0;
					return task::exit(code);
				}
				_ => ENOSYS,
			}
		}
	}
}

fn linux_write(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if buf_ptr == 0 {
        return EINVAL;
    }
    let len = len as usize;
    if len == 0 {
        return 0;
    }

    let bytes = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len) };
    let text = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return EINVAL,
    };

    match fd {
        1 | 2 => {
            crate::util::console::print(format_args!("{}", text));
            crate::util::vga::print(format_args!("{}", text));
            len as u64
        }
        _ => EINVAL,
    }
}

fn linux_read(_fd: u64, _buf_ptr: u64, _len: u64) -> u64 {
    ENOSYS
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
