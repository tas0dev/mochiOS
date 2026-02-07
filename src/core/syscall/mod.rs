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
        //   [元のrsp-8]   SS
        //   [元のrsp-16]  RSP
        //   [元のrsp-24]  RFLAGS
        //   [元のrsp-32]  CS
        //   [元のrsp-40]  RIP  ← この時点でrspはここを指す
        //
        // その後、ハンドラでpush順: rax, rcx, rdx, rbx, rbp, rsi, rdi, r8, r9, r10, r11, r12, r13, r14, r15
        //
        // 最終的なスタックレイアウト:
        // [rsp+0]   = r15  (最後にpush)
        // [rsp+8]   = r14
        // [rsp+16]  = r13
        // [rsp+24]  = r12
        // [rsp+32]  = r11
        // [rsp+40]  = r10 (arg3)
        // [rsp+48]  = r9
        // [rsp+56]  = r8  (arg4)
        // [rsp+64]  = rdi (arg0)
        // [rsp+72]  = rsi (arg1)
        // [rsp+80]  = rbp
        // [rsp+88]  = rbx
        // [rsp+96]  = rdx (arg2)
        // [rsp+104] = rcx
        // [rsp+112] = rax (syscall number) (最初にpush)
        // [rsp+120] = RIP (CPUが自動pushしたもの)
        // [rsp+128] = CS
        // [rsp+136] = RFLAGS
        // [rsp+144] = RSP
        // [rsp+152] = SS

        // カーネルデータセグメントをロード
        // （ds/esはスタックに保存しない。復元時にユーザーセグメントを再設定）
        "mov ax, 0x10",    // カーネルデータセグメント (index=2)
        "mov ds, ax",
        "mov es, ax",

        // システムコール引数を Rust 関数に渡す (System V ABI)
        // 引数: (num, arg0, arg1, arg2, arg3, arg4)
        // ユーザーランドのレジスタ配置: (rax=num, rdi=arg0, rsi=arg1, rdx=arg2, r10=arg3, r8=arg4)
        // カーネル関数の引数: (rdi=num, rsi=arg0, rdx=arg1, rcx=arg2, r8=arg3, r9=arg4)

        "mov rdi, [rsp + 112]",  // rax (syscall number) -> rdi
        "mov rsi, [rsp + 64]",   // rdi (arg0) -> rsi
        "mov rdx, [rsp + 72]",   // rsi (arg1) -> rdx
        "mov rcx, [rsp + 96]",   // rdx (arg2) -> rcx
        "mov r8,  [rsp + 40]",   // r10 (arg3) -> r8
        "mov r9,  [rsp + 56]",   // r8  (arg4) -> r9

        // スタックを16バイトアラインメント（System V ABI要件）
        // 現在 rsp は 16の倍数 + 8 なので、8引く
        "sub rsp, 8",

        // Rust 関数を呼び出し
        "call {syscall_handler}",

        // スタックを戻す
        "add rsp, 8",

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

/* デバッグ用にコメントアウト
/// デバッグ用: スタックの内容をダンプ
extern "C" fn dump_syscall_stack(stack_ptr: u64) {
    use crate::debug;

    unsafe {
        let ptr = stack_ptr as *const u64;
        debug!("Stack dump:");
        debug!("  [rsp+0]   (es)  = {:#x}", *ptr.offset(0));
        debug!("  [rsp+8]   (ds)  = {:#x}", *ptr.offset(1));
        debug!("  [rsp+16]  (r15) = {:#x}", *ptr.offset(2));
        debug!("  [rsp+24]  (r14) = {:#x}", *ptr.offset(3));
        debug!("  [rsp+32]  (r13) = {:#x}", *ptr.offset(4));
        debug!("  [rsp+40]  (r12) = {:#x}", *ptr.offset(5));
        debug!("  [rsp+48]  (r11) = {:#x}", *ptr.offset(6));
        debug!("  [rsp+56]  (r10) = {:#x}", *ptr.offset(7));
        debug!("  [rsp+64]  (r9)  = {:#x}", *ptr.offset(8));
        debug!("  [rsp+72]  (r8)  = {:#x}", *ptr.offset(9));
        debug!("  [rsp+80]  (rdi) = {:#x}", *ptr.offset(10));
        debug!("  [rsp+88]  (rsi) = {:#x}", *ptr.offset(11));
        debug!("  [rsp+96]  (rbp) = {:#x}", *ptr.offset(12));
        debug!("  [rsp+104] (rbx) = {:#x}", *ptr.offset(13));
        debug!("  [rsp+112] (rdx) = {:#x}", *ptr.offset(14));
        debug!("  [rsp+120] (rcx) = {:#x}", *ptr.offset(15));
        debug!("  [rsp+128] (rax) = {:#x}", *ptr.offset(16));
    }
}
*/

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

    debug!("SYSCALL: num={}, args=[{:#x}, {:#x}, {:#x}]", num, arg0, arg1, arg2);


    let ret = dispatch(num, arg0, arg1, arg2, arg3, arg4);

    debug!("SYSCALL returned: {}", ret);

    ret
}
