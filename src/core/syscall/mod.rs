//! システムコール

pub mod ipc;
pub mod task;
pub mod time;
pub mod exec;
pub mod io;
pub mod process;
pub mod fs;
pub mod io_port;
pub mod syscall_entry;

mod types;

pub use types::{SyscallNumber, EAGAIN, EINVAL, ENOSYS, EBADF, EFAULT, ENOENT, EPERM, SUCCESS};

use core::arch::asm;
use x86_64::structures::idt::InterruptStackFrame;

/// システムコールのディスパッチ
pub fn dispatch(num: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> u64 {
    match num {
        // Linux互換syscall
        x if x == SyscallNumber::Read as u64         => io::read(arg0, arg1, arg2),
        x if x == SyscallNumber::Write as u64        => io::write(arg0, arg1, arg2),
        x if x == SyscallNumber::Open as u64         => fs::open(arg0, arg1),
        x if x == SyscallNumber::Close as u64        => fs::close(arg0),
        x if x == SyscallNumber::Fstat as u64        => fs::fstat(arg0, arg1),
        x if x == SyscallNumber::Lseek as u64        => fs::seek(arg0, arg1 as i64, arg2),
        x if x == SyscallNumber::Mmap as u64         => process::mmap(arg0, arg1, arg2, arg3, arg4),
        x if x == SyscallNumber::Munmap as u64       => process::munmap(arg0, arg1),
        x if x == SyscallNumber::Brk as u64          => process::brk(arg0),
        x if x == SyscallNumber::RtSigaction as u64  => SUCCESS,   // スタブ
        x if x == SyscallNumber::RtSigprocmask as u64 => SUCCESS,  // スタブ
        x if x == SyscallNumber::GetPid as u64       => process::getpid(),
        x if x == SyscallNumber::Clone as u64        => ENOSYS,    // TODO
        x if x == SyscallNumber::Fork as u64         => ENOSYS,    // TODO
        x if x == SyscallNumber::Wait as u64         => process::wait(arg0, arg1),
        x if x == SyscallNumber::GetTid as u64       => process::gettid(),
        x if x == SyscallNumber::Futex as u64        => process::futex(arg0, arg1 as u32, arg2, arg3),
        x if x == SyscallNumber::ArchPrctl as u64    => process::arch_prctl(arg0, arg1),
        x if x == SyscallNumber::ClockGettime as u64 => time::clock_gettime(arg0, arg1),
        x if x == SyscallNumber::Getcwd as u64       => fs::getcwd(arg0, arg1),
        x if x == SyscallNumber::Exit as u64         => process::exit(arg0),
        x if x == SyscallNumber::ExitGroup as u64    => process::exit(arg0),

        // SwiftCore独自syscall
        x if x == SyscallNumber::Yield as u64        => { task::yield_now(); SUCCESS },
        x if x == SyscallNumber::GetTicks as u64     => time::get_ticks(),
        x if x == SyscallNumber::IpcSend as u64      => ipc::send(arg0, arg1, arg2),
        x if x == SyscallNumber::IpcRecv as u64      => ipc::recv(arg0, arg1),
        x if x == SyscallNumber::Exec as u64         => exec::exec_kernel(arg0),
        x if x == SyscallNumber::Sleep as u64        => process::sleep(arg0),
        x if x == SyscallNumber::Log as u64          => io::log(arg0, arg1, arg2),
        x if x == SyscallNumber::PortIn as u64       => io_port::port_in(arg0, arg1),
        x if x == SyscallNumber::PortOut as u64      => io_port::port_out(arg0, arg1, arg2),
        x if x == SyscallNumber::Mkdir as u64        => fs::mkdir(arg0, arg1),
        x if x == SyscallNumber::Rmdir as u64        => fs::rmdir(arg0),
        x if x == SyscallNumber::Readdir as u64      => fs::readdir(arg0, arg1, arg2),
        x if x == SyscallNumber::Chdir as u64        => fs::chdir(arg0),
        x if x == SyscallNumber::FindProcessByName as u64 => process::find_process_by_name(arg0, arg1),
        _ => ENOSYS,
    }
}

/// システムコール割り込みハンドラ (int 0x80) - アセンブリラッパー
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

        // カーネルデータセグメントをロード
        // （ds/esはスタックに保存しない。復元時にユーザーセグメントを再設定）
        "mov ax, 0x10",    // カーネルデータセグメント (index=2)
        "mov ds, ax",
        "mov es, ax",

        "sub rsp, 48",

        // スタック上の引数を設定 (arg3, arg4)
        "mov r11, [rsp + 48 + 40]", // r10 (arg3)
        "mov [rsp + 32], r11",
        "mov r11, [rsp + 48 + 56]", // u_r8 (arg4)
        "mov [rsp + 40], r11",

        // レジスタ引数を設定 (num, arg0, arg1, arg2)
        "mov rcx, [rsp + 48 + 112]", // rax (num)
        "mov rdx, [rsp + 48 + 64]",  // rdi (arg0)
        "mov r8,  [rsp + 48 + 72]",  // rsi (arg1)
        "mov r9,  [rsp + 48 + 96]",  // rdx (arg2)

        // Rust 関数を呼び出し
        "call {syscall_handler}",

        // スタックを戻す
        "add rsp, 48",

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
    use crate::debug;
    use core::sync::atomic::Ordering;

    // syscall中（カーネルモード）でもCR3はユーザーのページテーブルのまま。
    // カーネルが全物理メモリにアクセスできるよう、カーネルのCR3に切り替える。
    let user_cr3 = {
        let (frame, _) = x86_64::registers::control::Cr3::read();
        let phys = frame.start_address().as_u64();
        let kernel_l4 = crate::mem::paging::KERNEL_L4_PHYS.load(Ordering::Relaxed);
        if kernel_l4 != 0 && kernel_l4 != phys {
            unsafe { crate::mem::paging::switch_page_table(kernel_l4); }
        }
        phys
    };

    debug!("SYSCALL: num={}, args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}]", 
           num, arg0, arg1, arg2, arg3, arg4);

    let ret = dispatch(num, arg0, arg1, arg2, arg3, arg4);

    debug!("SYSCALL returned: {}", ret);

    // ユーザーのCR3に戻す（カーネルのCR3と異なる場合のみ）
    {
        let kernel_l4 = crate::mem::paging::KERNEL_L4_PHYS.load(Ordering::Relaxed);
        if user_cr3 != 0 && user_cr3 != kernel_l4 {
            unsafe { crate::mem::paging::switch_page_table(user_cr3); }
        }
    }

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
