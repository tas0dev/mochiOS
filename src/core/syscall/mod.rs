//! システムコール

pub mod exec;
pub mod fs;
pub mod io;
pub mod io_port;
pub mod ipc;
pub mod keyboard;
pub mod process;
pub mod syscall_entry;
pub mod task;
pub mod time;
pub mod vga;

mod types;
mod console;
mod linux;

use alloc::string::String;
use alloc::vec::Vec;

/// ユーザー空間ポインタの有効性を検証する
///
/// ポインタが null でなく、ユーザー空間のアドレス範囲内にあること、
/// かつ `ptr + len` がオーバーフローしないことを確認する。
///
/// x86-64 canonical ユーザー空間上限: 0x0000_7FFF_FFFF_FFFF
pub fn validate_user_ptr(ptr: u64, len: u64) -> bool {
    if ptr == 0 {
        return false;
    }
    // x86-64 ユーザー空間の上限アドレス (canonical hole 下側)
    const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;
    if ptr > USER_SPACE_END {
        return false;
    }
    let end_inclusive = if len == 0 {
        ptr
    } else {
        match ptr.checked_add(len - 1) {
            Some(e) => e,
            None => return false, // 整数オーバーフロー
        }
    };
    if end_inclusive > USER_SPACE_END {
        return false;
    }

    let user_pt = match crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
        .and_then(|pid| crate::task::with_process(pid, |p| p.page_table()))
        .flatten()
    {
        Some(pt) => pt,
        None => return false,
    };

    crate::mem::paging::is_user_range_mapped_in_table(user_pt, ptr, len)
}

/// ユーザー空間の null 終端文字列を最大長付きで読み取り、カーネル所有の `String` を返す。
pub fn read_user_cstring(ptr: u64, max_len: usize) -> Result<String, u64> {
    if ptr == 0 || max_len == 0 {
        return Err(EINVAL);
    }
    if !validate_user_ptr(ptr, 1) {
        return Err(EFAULT);
    }

    let mut bytes = Vec::with_capacity(max_len);
    let mut checked_page = u64::MAX;
    for i in 0..max_len {
        let addr = ptr.checked_add(i as u64).ok_or(EFAULT)?;
        let page_base = addr & !0xfffu64;
        if page_base != checked_page {
            if !validate_user_ptr(addr, 1) {
                return Err(EFAULT);
            }
            checked_page = page_base;
        }
        let b = with_user_memory_access(|| unsafe { core::ptr::read(addr as *const u8) });
        if b == 0 {
            return String::from_utf8(bytes).map_err(|_| EINVAL);
        }
        bytes.push(b);
    }
    Err(EINVAL)
}

/// ユーザー空間からバイト列をコピーする（コピー先はカーネル空間）。
pub fn copy_from_user(src_ptr: u64, dst: &mut [u8]) -> Result<(), u64> {
    if dst.is_empty() {
        return Ok(());
    }
    if src_ptr == 0 {
        return Err(EFAULT);
    }
    if !validate_user_ptr(src_ptr, dst.len() as u64) {
        return Err(EFAULT);
    }

    for (i, out) in dst.iter_mut().enumerate() {
        let addr = src_ptr.checked_add(i as u64).ok_or(EFAULT)?;
        *out = with_user_memory_access(|| unsafe { core::ptr::read(addr as *const u8) });
    }
    Ok(())
}

/// ユーザーポインタを実際に参照する短い区間を、必要に応じてユーザーCR3で実行する。
///
/// KPTI有効時、syscall本体はkernel CR3で実行されるため、ユーザー仮想アドレスを
/// 直接参照する区間だけ一時的にuser CR3へ切り替える。
pub fn with_user_memory_access<R>(f: impl FnOnce() -> R) -> R {
    use x86_64::registers::control::Cr3;
    x86_64::instructions::interrupts::without_interrupts(|| {
        let kernel_cr3 = crate::percpu::kernel_cr3();
        if kernel_cr3 == 0 {
            return f();
        }

        let (cur, _) = Cr3::read();
        let current_cr3 = cur.start_address().as_u64();
        if current_cr3 != kernel_cr3 {
            return f();
        }

        let user_pt = crate::task::current_thread_id()
            .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
            .and_then(|pid| crate::task::with_process(pid, |p| p.page_table()))
            .flatten()
            .unwrap_or(0);
        if user_pt == 0 {
            return f();
        }

        if crate::cpu::is_smap_enabled() {
            unsafe {
                asm!("stac", options(nostack, preserves_flags));
            }
        }
        crate::mem::paging::switch_page_table(user_pt);
        let out = f();
        crate::mem::paging::switch_page_table(kernel_cr3);
        if crate::cpu::is_smap_enabled() {
            unsafe {
                asm!("clac", options(nostack, preserves_flags));
            }
        }
        out
    })
}

pub use types::{
    SyscallNumber, EAGAIN, EBADF, EFAULT, EINVAL, ENODATA, ENOENT, ENOSYS, EPERM, SUCCESS,
};

use core::arch::asm;
use x86_64::structures::idt::InterruptStackFrame;

/// システムコールのディスパッチ
pub fn dispatch(num: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> u64 {
    match num {
        // Linux互換syscall
        x if x == SyscallNumber::Read as u64 => io::read(arg0, arg1, arg2),
        x if x == SyscallNumber::Write as u64 => io::write(arg0, arg1, arg2),
        x if x == SyscallNumber::Open as u64 => fs::open(arg0, arg1),
        x if x == SyscallNumber::Close as u64 => fs::close(arg0),
        x if x == SyscallNumber::Stat as u64 => fs::stat(arg0, arg1),
        x if x == SyscallNumber::Fstat as u64 => fs::fstat(arg0, arg1),
        x if x == SyscallNumber::Lseek as u64 => fs::seek(arg0, arg1 as i64, arg2),
        x if x == SyscallNumber::Mmap as u64 => process::mmap(arg0, arg1, arg2, arg3, arg4),
        x if x == SyscallNumber::Munmap as u64 => process::munmap(arg0, arg1),
        x if x == SyscallNumber::Brk as u64 => process::brk(arg0),
        x if x == SyscallNumber::RtSigaction as u64 => ENOSYS,
        x if x == SyscallNumber::RtSigprocmask as u64 => ENOSYS,
        x if x == SyscallNumber::GetPid as u64 => process::getpid(),
        x if x == SyscallNumber::Clone as u64 => process::fork(),
        x if x == SyscallNumber::Fork as u64 => process::fork(),
        x if x == SyscallNumber::Execve as u64 => exec::execve_syscall(arg0, arg1, arg2),
        x if x == SyscallNumber::Wait as u64 => process::wait(arg0, arg1, arg2),
        x if x == SyscallNumber::GetTid as u64 => process::gettid(),
        x if x == SyscallNumber::Futex as u64 => process::futex(arg0, arg1 as u32, arg2, arg3),
        x if x == SyscallNumber::ArchPrctl as u64 => process::arch_prctl(arg0, arg1),
        x if x == SyscallNumber::ClockGettime as u64 => time::clock_gettime(arg0, arg1),
        x if x == SyscallNumber::Getcwd as u64 => fs::getcwd(arg0, arg1),
        x if x == SyscallNumber::Exit as u64 => process::exit(arg0),
        x if x == SyscallNumber::ExitGroup as u64 => process::exit(arg0),

        // mochiOS独自syscall
        x if x == SyscallNumber::Yield as u64 => {
            task::yield_now();
            SUCCESS
        }
        x if x == SyscallNumber::GetTicks as u64 => time::get_ticks(),
        x if x == SyscallNumber::IpcSend as u64 => ipc::send(arg0, arg1, arg2),
        x if x == SyscallNumber::IpcRecv as u64 => ipc::recv(arg0, arg1),
        x if x == SyscallNumber::Exec as u64 => exec::exec_kernel(arg0),
        x if x == SyscallNumber::Sleep as u64 => process::sleep(arg0),
        x if x == SyscallNumber::Log as u64 => io::log(arg0, arg1, arg2),
        x if x == SyscallNumber::PortIn as u64 => io_port::port_in(arg0, arg1),
        x if x == SyscallNumber::PortOut as u64 => io_port::port_out(arg0, arg1, arg2),
        x if x == SyscallNumber::Mkdir as u64 => fs::mkdir(arg0, arg1),
        x if x == SyscallNumber::Rmdir as u64 => fs::rmdir(arg0),
        x if x == SyscallNumber::Readdir as u64 => fs::readdir(arg0, arg1, arg2),
        x if x == SyscallNumber::Chdir as u64 => fs::chdir(arg0),
        x if x == SyscallNumber::KeyboardRead as u64 => keyboard::read_char(),
        x if x == SyscallNumber::FindProcessByName as u64 => {
            process::find_process_by_name(arg0, arg1)
        }
        x if x == SyscallNumber::GetThreadPrivilege as u64 => task::get_thread_privilege(arg0),
        x if x == SyscallNumber::GetFramebufferInfo as u64 => vga::get_framebuffer_info(arg0),
        x if x == SyscallNumber::MapFramebuffer as u64 => vga::map_framebuffer(),
        x if x == SyscallNumber::ExecFromBuffer as u64 => exec::exec_from_buffer_syscall(arg0, arg1),
        _ => ENOSYS,
    }
}

/// fork/clone のみ、現在スレッドへユーザーコンテキストを保存する
#[no_mangle]
pub extern "sysv64" fn save_user_context_for_fork(
    num: u64,
    user_rip: u64,
    user_rsp: u64,
    user_rflags: u64,
) {
    if num != SyscallNumber::Clone as u64 && num != SyscallNumber::Fork as u64 {
        return;
    }
    if let Some(tid) = crate::task::current_thread_id() {
        crate::task::with_thread_mut(tid, |t| {
            t.set_syscall_user_context(user_rip, user_rsp, user_rflags);
        });
    }
}

/// システムコール割り込みハンドラ (int 0x80) - アセンブリラッパー
///
/// # Safety
/// CPU が int 0x80 入口規約どおりのスタック/レジスタ状態でこの関数へ入ることを前提とする。
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn syscall_interrupt_handler() {
    core::arch::naked_asm!(
        // すべてのレジスタを保存（システムコール引数を含む）
        "push rax",      // syscall number
        "push rcx",
        "push rdx",      // arg2
        "push rbx",
        "push rbp",
        "push rsi",      // arg1
        "push rdi",      // arg0
        "push r8",       // arg4
        "push r9",
        "push r10",      // arg3
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // fork/clone のときだけ、ユーザーコンテキストを現在スレッドへ保存
        // saved stack layout:
        // [rsp+112]=num(rax), [rsp+120]=user RIP, [rsp+136]=user RFLAGS, [rsp+144]=user RSP
        "mov rax, [rsp + 112]",
        "cmp rax, 56",
        "je 2f",
        "cmp rax, 57",
        "jne 3f",
        "2:",
        "mov rdi, rax",
        "mov rsi, [rsp + 120]",
        "mov rdx, [rsp + 144]",
        "mov rcx, [rsp + 136]",
        "call {save_ctx_fn}",
        "3:",

        // カーネルデータセグメントをロード
        // （ds/esはスタックに保存しない。復元時にユーザーセグメントを再設定）
        "mov ax, 0x10",    // カーネルデータセグメント (index=2)
        "mov ds, ax",
        "mov es, ax",

        // System V AMD64 ABI: rdi=num, rsi=arg0, rdx=arg1, rcx=arg2, r8=arg3, r9=arg4
        // スタック上のオフセット (15 pushes × 8 bytes, sub rsp なし):
        //   [rsp+0]=r15, [rsp+8]=r14, [rsp+16]=r13, [rsp+24]=r12, [rsp+32]=r11,
        //   [rsp+40]=r10(arg3), [rsp+48]=r9, [rsp+56]=r8(arg4),
        //   [rsp+64]=rdi(arg0), [rsp+72]=rsi(arg1), [rsp+80]=rbp, [rsp+88]=rbx,
        //   [rsp+96]=rdx(arg2), [rsp+104]=rcx, [rsp+112]=rax(num)
        "mov rdi, [rsp + 112]", // rax (syscall number)
        "mov rsi, [rsp + 64]",  // rdi (arg0)
        "mov rdx, [rsp + 72]",  // rsi (arg1)
        "mov rcx, [rsp + 96]",  // rdx (arg2)
        "mov r8,  [rsp + 40]",  // r10 (arg3)
        "mov r9,  [rsp + 56]",  // r8  (arg4)

        // Rust 関数を呼び出し (16バイトアライン済み: 160バイトオフセット)
        "call {syscall_handler}",

        // 戻り値 (rax) をスタック上の保存された rax の位置に書き込む
        "mov [rsp + 112], rax",

        // ユーザーデータセグメントを設定
        "mov ax, 0x1b",    // ユーザーデータセグメント (index=3, RPL=3)
        "mov ds, ax",
        "mov es, ax",

        // すべてのレジスタを復元
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rbx",
        "pop rdx",
        "pop rcx",
        "pop rax",

        // 割り込みから戻る
        "iretq",

        save_ctx_fn = sym save_user_context_for_fork,
        syscall_handler = sym syscall_handler_rust,
    );
}

/// システムコールハンドラの Rust 実装
extern "C" fn syscall_handler_rust(
    num: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
) -> u64 {
    let current_tid = crate::task::current_thread_id();
    let prev_cr3 = syscall_entry::switch_to_kernel_page_table();
    if let Some(tid) = current_tid {
        crate::task::with_thread_mut(tid, |t| t.set_in_syscall(true));
    }
    let ret = dispatch(num, arg0, arg1, arg2, arg3, arg4);
    if let Some(tid) = current_tid {
        crate::task::with_thread_mut(tid, |t| t.set_in_syscall(false));
    }
    syscall_entry::restore_page_table(prev_cr3);
    ret
}

/// SYSCALL 命令エントリから呼ばれる System V ABI ディスパッチ関数
///
/// syscall_entry.rs の naked asm から `call {dispatch}` で呼ばれる。
/// System V ABI: 引数は rdi, rsi, rdx, rcx, r8, r9 の順。
#[no_mangle]
pub extern "sysv64" fn syscall_dispatch_sysv(
    num: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
) -> u64 {
    syscall_handler_rust(num, arg0, arg1, arg2, arg3, arg4)
}
