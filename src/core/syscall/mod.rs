//! システムコール

pub mod ipc;
pub mod task;
pub mod time;
pub mod exec;
pub mod io;
pub mod process;
pub mod fs;

mod types;

pub use types::{SyscallNumber, EAGAIN, EINVAL, ENOSYS, EBADF, EFAULT, ENOENT, EPERM, SUCCESS};

use core::arch::asm;
use x86_64::structures::idt::InterruptStackFrame;

/// システムコールのディスパッチ
pub fn dispatch(num: u64, arg0: u64, arg1: u64, arg2: u64, _arg3: u64, _arg4: u64) -> u64 {
    match num {
        x if x == SyscallNumber::Yield as u64 => task::yield_now(),
        x if x == SyscallNumber::GetTicks as u64 => time::get_ticks(),
        x if x == SyscallNumber::IpcSend as u64 => ipc::send(arg0, arg1),
        x if x == SyscallNumber::IpcRecv as u64 => ipc::recv(arg0),
        x if x == SyscallNumber::Exec as u64 => exec::exec_kernel(arg0),
        x if x == SyscallNumber::Write as u64 => io::write(arg0, arg1, arg2),
        x if x == SyscallNumber::Read as u64 => io::read(arg0, arg1, arg2),
        x if x == SyscallNumber::Exit as u64 => process::exit(arg0),
        x if x == SyscallNumber::GetPid as u64 => process::getpid(),
        x if x == SyscallNumber::GetTid as u64 => process::gettid(),
        x if x == SyscallNumber::Sleep as u64 => process::sleep(arg0),
        x if x == SyscallNumber::Open as u64 => fs::open(arg0, arg1),
        x if x == SyscallNumber::Close as u64 => fs::close(arg0),
        x if x == SyscallNumber::Fork as u64 => process::fork(),
        x if x == SyscallNumber::Wait as u64 => process::wait(arg0, arg1),
        _ => ENOSYS,
    }
}

/// システムコール割り込みハンドラ (int 0x80) - アセンブリラッパー
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn syscall_interrupt_handler() {
    core::arch::naked_asm!(
        // int 0x80が呼ばれた時点で、CPUが自動的にスタックにプッシュ:
        // [rsp+32] SS
        // [rsp+24] RSP
        // [rsp+16] RFLAGS
        // [rsp+8]  CS
        // [rsp+0]  RIP

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

        // push完了後のスタックレイアウト (rsp からのオフセット):
        //
        // int 0x80が呼ばれた時点で、CPUが自動的にスタックにプッシュ（合計40バイト）:
        //   [R0-8]   RIP
        //   [R0-16]  CS
        //   [R0-24]  RFLAGS
        //   [R0-32]  RSP (ユーザーのrsp)
        //   [R0-40]  SS  ← R0 = 元のrsp
        //
        // その後、ハンドラでpush順: rax, rcx, rdx, rbx, rbp, rsi, rdi, r8, r9, r10, r11, r12, r13, r14, r15
        //
        // 最終的なスタックレイアウト (rsp = R0-160):
        // [rsp+0]   = r15  (最後にpush) [R0-160]
        // [rsp+8]   = r14              [R0-152]
        // [rsp+16]  = r13              [R0-144]
        // [rsp+24]  = r12              [R0-136]
        // [rsp+32]  = r11              [R0-128]
        // [rsp+40]  = r10 (arg3)       [R0-120]
        // [rsp+48]  = r9               [R0-112]
        // [rsp+56]  = r8  (arg4)       [R0-104]
        // [rsp+64]  = rdi (arg0)       [R0-96]
        // [rsp+72]  = rsi (arg1)       [R0-88]
        // [rsp+80]  = rbp              [R0-80]
        // [rsp+88]  = rbx              [R0-72]
        // [rsp+96]  = rdx (arg2)       [R0-64]
        // [rsp+104] = rcx              [R0-56]
        // [rsp+112] = rax (syscall #)  [R0-48] (最初にpush)
        // [rsp+120] = SS               [R0-40]
        // [rsp+128] = RSP              [R0-32]
        // [rsp+136] = RFLAGS           [R0-24]
        // [rsp+144] = CS               [R0-16]
        // [rsp+152] = RIP              [R0-8]

        // カーネルデータセグメントをロード
        // （ds/esはスタックに保存しない。復元時にユーザーセグメントを再設定）
        "mov ax, 0x10",    // カーネルデータセグメント (index=2)
        "mov ds, ax",
        "mov es, ax",

        // デバッグ: スタックダンプ (MS ABI: Arg1 in RCX)
        "mov rcx, rsp",      // Arg1: pointer to saved registers
        "sub rsp, 32",       // Shadow space (32 bytes)
        "call {dump_stack}",
        "add rsp, 32",       // Cleanup shadow space

        // システムコール引数を Rust 関数に渡す (Microsoft x64 ABI for UEFI)
        // 引数: (num, arg0, arg1, arg2, arg3, arg4)
        // MS ABI: RCX, RDX, R8, R9, Stack, Stack
        // ユーザー入力 (SysV-like):
        //   num (RAX)  -> RCX
        //   arg0 (RDI) -> RDX
        //   arg1 (RSI) -> R8
        //   arg2 (RDX) -> R9
        //   arg3 (R10) -> Stack[rsp+32]
        //   arg4 (R8)  -> Stack[rsp+40]

        // Stack Frame for Call: 32 (Shadow) + 16 (Args) = 48 bytes
        // RSP must be 16-byte aligned before CALL.
        // Current RSP is aligned (160 bytes pushed). 48 is divisible by 16.
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
        dump_stack = sym dump_syscall_stack,
    );
}

// デバッグ用にコメントアウト
/// デバッグ用: スタックの内容をダンプ
pub extern "C" fn dump_syscall_stack(stack_ptr: u64) {
    use crate::debug;

    unsafe {
        let ptr = stack_ptr as *const u64;
        debug!("Stack dump (rsp base={:#x}):", stack_ptr);
        debug!("  [rsp+0]   (r15) = {:#x}", *ptr.offset(0));
        debug!("  [rsp+8]   (r14) = {:#x}", *ptr.offset(1));
        debug!("  [rsp+16]  (r13) = {:#x}", *ptr.offset(2));
        debug!("  [rsp+24]  (r12) = {:#x}", *ptr.offset(3));
        debug!("  [rsp+32]  (r11) = {:#x}", *ptr.offset(4));
        debug!("  [rsp+40]  (r10) = {:#x}", *ptr.offset(5));
        debug!("  [rsp+48]  (r9)  = {:#x}", *ptr.offset(6));
        debug!("  [rsp+56]  (r8)  = {:#x}", *ptr.offset(7));
        debug!("  [rsp+64]  (rdi) = {:#x}", *ptr.offset(8));
        debug!("  [rsp+72]  (rsi) = {:#x}", *ptr.offset(9));
        debug!("  [rsp+80]  (rbp) = {:#x}", *ptr.offset(10));
        debug!("  [rsp+88]  (rbx) = {:#x}", *ptr.offset(11));
        debug!("  [rsp+96]  (rdx) = {:#x}", *ptr.offset(12));
        debug!("  [rsp+104] (rcx) = {:#x}", *ptr.offset(13));
        debug!("  [rsp+112] (rax) = {:#x}", *ptr.offset(14));
        debug!("  [rsp+120] (SS)  = {:#x}", *ptr.offset(15));
        debug!("  [rsp+128] (rsp) = {:#x}", *ptr.offset(16));
        debug!("  [rsp+136] (flg) = {:#x}", *ptr.offset(17));
        debug!("  [rsp+144] (cs)  = {:#x}", *ptr.offset(18));
        debug!("  [rsp+152] (rip) = {:#x}", *ptr.offset(19));
    }
}
// */

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

    debug!("SYSCALL: num={}, args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}]", 
           num, arg0, arg1, arg2, arg3, arg4);

    let ret = dispatch(num, arg0, arg1, arg2, arg3, arg4);

    debug!("SYSCALL returned: {}", ret);

    ret
}
