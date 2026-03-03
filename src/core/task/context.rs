use crate::task::ids::ThreadId;
use crate::task::process::with_process;
use crate::task::thread::THREAD_QUEUE;

/// CPU コンテキスト（callee-saved 等を保存）
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Context {
    pub rsp: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rdi: u64, // MS ABI Callee-saved
    pub rsi: u64, // MS ABI Callee-saved
    /// 命令ポインタ（戻り先アドレス）
    pub rip: u64,
    pub rflags: u64,
}

impl Context {
    pub const fn new() -> Self {
        Self {
            rsp: 0,
            rbp: 0,
            rbx: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rdi: 0,
            rsi: 0,
            rip: 0,
            rflags: 0,
        }
    }
}

/// 初回スイッチ時に使用するダミーコンテキスト（保存先として使われるが値は参照されない）
static mut INITIAL_DUMMY_CONTEXT: Context = Context::new();
/// 現在のスレッドから次のスレッドへコンテキストを切り替える
///
/// Context構造体のレイアウト:
/// offset 0x00: rsp
/// offset 0x08: rbp
/// offset 0x10: rbx
/// offset 0x18: r12
/// offset 0x20: r13
/// offset 0x28: r14
/// offset 0x30: r15
/// offset 0x38: rdi
/// offset 0x40: rsi
/// offset 0x48: rip
/// offset 0x50: rflags
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn switch_context(old_context: *mut Context, new_context: *const Context) {
    core::arch::naked_asm!(
        "cli",
        // save current (ret address is at [rsp])
        "lea rax, [rsp + 0x08]",
        // Microsoft x64 ABI (used by x86_64-unknown-uefi):
        // 第1引数 (old_context) = rcx
        // 第2引数 (new_context) = rdx
        "mov [rcx + 0x00], rax", // rsp
        "mov [rcx + 0x08], rbp", // rbp
        "mov [rcx + 0x10], rbx", // rbx
        "mov [rcx + 0x18], r12", // r12
        "mov [rcx + 0x20], r13", // r13
        "mov [rcx + 0x28], r14", // r14
        "mov [rcx + 0x30], r15", // r15
        "mov [rcx + 0x38], rdi", // rdi (MS ABI Callee-saved)
        "mov [rcx + 0x40], rsi", // rsi (MS ABI Callee-saved)
        // 戻り先アドレス（call命令でスタックにpushされている）を保存
        "mov rax, [rsp]",
        "mov [rcx + 0x48], rax", // rip
        // RFLAGSを保存
        "pushfq",
        "pop rax",
        "mov [rcx + 0x50], rax", // rflags
        // 新しいコンテキストを復元
        "mov rax, [rdx + 0x48]", // 新しいrip
        "mov r11, [rdx + 0x50]", // 新しいrflags
        "mov rbx, [rdx + 0x10]", // rbx
        "mov r12, [rdx + 0x18]", // r12
        "mov r13, [rdx + 0x20]", // r13
        "mov r14, [rdx + 0x28]", // r14
        "mov r15, [rdx + 0x30]", // r15
        "mov rdi, [rdx + 0x38]", // rdi
        "mov rsi, [rdx + 0x40]", // rsi
        "mov rbp, [rdx + 0x08]", // rbp
        "mov rsp, [rdx + 0x00]", // rsp
        // RFLAGSを復元
        "push r11",
        "popfq",
        "jmp rax",
    );
}

/// 別スレッドへ切替（通常呼び出し経路）
pub unsafe fn switch_to_thread(current_id: Option<ThreadId>, next_id: ThreadId) {
    // コンテキストスイッチ中は割り込みを禁止する
    // ロック解放からコンテキストスイッチまでの間に割り込みが入ると不整合が起きる可能性があるため
    x86_64::instructions::interrupts::disable();

    crate::debug!(
        "switch_to_thread: current_id={:?}, next_id={:?}",
        current_id,
        next_id
    );

    let mut queue = THREAD_QUEUE.lock();

    let old_ctx_ptr = if let Some(id) = current_id {
        if let Some(thread) = queue.get_mut(id) {
            let ptr = thread.context_mut() as *mut Context;
            crate::debug!(
                "  Current context ptr: {:p}, rsp={:#x}, rip={:#x}",
                ptr,
                thread.context().rsp,
                thread.context().rip
            );
            ptr
        } else {
            return; // 現在のスレッドが見つからない
        }
    } else {
        // 現在のスレッドがない場合（初回スイッチ）はダミーに書き込む（値は捨てられる）
        crate::debug!("  No current thread (initial switch)");
        unsafe { core::ptr::addr_of_mut!(INITIAL_DUMMY_CONTEXT) }
    };

    // 次のスレッドのコンテキストへのポインタとカーネルスタックトップを取得
    let (new_context_ptr, next_kstack_top, next_process_id, next_fs_base) =
        if let Some(thread) = queue.get(next_id) {
            let ptr = thread.context() as *const Context;
            let kstack = thread.kernel_stack_top();
            let pid = thread.process_id();
            let fs = thread.fs_base();
            crate::debug!(
                "  Next context ptr: {:p}, rsp={:#x}, rip={:#x}, kstack={:#x}",
                ptr,
                thread.context().rsp,
                thread.context().rip,
                kstack
            );
            (ptr, kstack, pid, fs)
        } else {
            return; // 次のスレッドが見つからない
        };

    drop(queue);

    // TSSのRSP0とSYSCALL用カーネルスタックを更新
    crate::mem::tss::set_rsp0(next_kstack_top);
    crate::syscall::syscall_entry::update_kernel_rsp(next_kstack_top);

    // 次のスレッドの FS ベースを復元 (TLS)
    if next_fs_base != 0 {
        unsafe {
            crate::cpu::write_fs_base(next_fs_base);
        }
    }

    // 次のプロセスのページテーブルに切り替え
    if let Some(pt_phys) = crate::task::with_process(next_process_id, |p| p.page_table()).flatten()
    {
        crate::mem::paging::switch_page_table(pt_phys);
    }

    crate::debug!("About to perform context switch...");
    switch_context(old_ctx_ptr, new_context_ptr);
}

/// カーネルから直接ユーザーモードに入るためのヘルパ（最初のユーザスレッド用）
pub unsafe fn enter_user_from_kernel(ctx: &Context) -> ! {
    let user_cs = crate::mem::gdt::user_code_selector() as u64;
    let user_ds = crate::mem::gdt::user_data_selector() as u64;

    core::arch::asm!(
        "cli",
        "mov rbx, {rbx}",
        "mov r12, {r12}",
        "mov r13, {r13}",
        "mov r14, {r14}",
        "mov r15, {r15}",
        "mov rbp, {rbp}",
        // iretq が CS/RIP/RFLAGS->(RSP/SS) を期待するので順に push
        "push {ss}",
        "push {user_rsp}",
        "push {rflags}",
        "push {cs}",
        "push {rip}",
        // restore user GS base from IA32_KERNEL_GS_BASE
        "swapgs",
        "iretq",
        rbx = in(reg) ctx.rbx,
        r12 = in(reg) ctx.r12,
        r13 = in(reg) ctx.r13,
        r14 = in(reg) ctx.r14,
        r15 = in(reg) ctx.r15,
        rbp = in(reg) ctx.rbp,
        ss = in(reg) user_ds as u64,
        user_rsp = in(reg) ctx.rsp,
        rflags = in(reg) ctx.rflags,
        cs = in(reg) user_cs,
        rip = in(reg) ctx.rip,
        options(noreturn)
    );
}

/// 割込み内からの切替。呼び出し側で割込み時のレジスタを `saved` に収めて渡す。
pub unsafe fn switch_to_thread_from_isr(
    current_id: Option<ThreadId>,
    next_id: ThreadId,
    saved: Context,
) {
    crate::debug!(
        "switch_to_thread_from_isr: current={:?}, next={:?}",
        current_id,
        next_id
    );

    let mut queue = THREAD_QUEUE.lock();

    let old_ctx_ptr = if let Some(id) = current_id {
        if let Some(thread) = queue.get_mut(id) {
            thread.context_mut() as *mut Context
        } else {
            return;
        }
    } else {
        unsafe { core::ptr::addr_of_mut!(INITIAL_DUMMY_CONTEXT) }
    };

    let (new_ctx_ptr, next_priv, next_kstack_top, next_fs_base, next_process_id) =
        if let Some(thread) = queue.get(next_id) {
            let ptr = thread.context() as *const Context;
            let proc = thread.process_id();
            let priv_level = crate::task::with_process(proc, |p| p.privilege())
                .unwrap_or(crate::task::PrivilegeLevel::Core);
            let kstack = thread.kernel_stack_top();
            let fs = thread.fs_base();
            (ptr, priv_level, kstack, fs, proc)
        } else {
            return;
        };

    if !old_ctx_ptr.is_null() {
        unsafe {
            *old_ctx_ptr = saved;
        }
    }

    drop(queue);

    // TSSのRSP0を更新
    crate::mem::tss::set_rsp0(next_kstack_top);

    // SYSCALL用カーネルスタックも更新 (次のスレッドのカーネルスタックを使う)
    crate::syscall::syscall_entry::update_kernel_rsp(next_kstack_top);

    // 次のスレッドの FS ベースを復元 (TLS)
    if next_fs_base != 0 {
        crate::cpu::write_fs_base(next_fs_base);
    }

    // 次のプロセスのページテーブルに切り替え
    if let Some(pt_phys) = crate::task::with_process(next_process_id, |p| p.page_table()).flatten()
    {
        crate::mem::paging::switch_page_table(pt_phys);
    }

    crate::debug!("About to perform context switch...");

    if next_priv == crate::task::PrivilegeLevel::Core {
        core::arch::asm!(
            "cli",
            "mov rsp, rax",       // rsp = saved.rsp
            "mov rbp, {rbp_val}", // Restore rbp
            "mov rbx, {rbx_val}", // Restore rbx
            "push rcx",           // push rflags
            "popfq",              // restore rflags
            "jmp rdx",            // jump to rip

            // Fixed registers
            in("rax") saved.rsp,
            in("rcx") saved.rflags,
            in("rdx") saved.rip,

            in("r12") saved.r12,
            in("r13") saved.r13,
            in("r14") saved.r14,
            in("r15") saved.r15,
            in("rdi") saved.rdi,
            in("rsi") saved.rsi,

            // Compiler allocated registers (will use r8-r11)
            rbp_val = in(reg) saved.rbp,
            rbx_val = in(reg) saved.rbx,

            options(noreturn)
        );
    } else {
        // ユーザーモードへ iretq で遷移するための準備
        let user_cs = crate::mem::gdt::user_code_selector() as u64;
        let user_ds = crate::mem::gdt::user_data_selector() as u64;

        core::arch::asm!(
            "cli",
            "mov rbx, {rbx}",
            "mov r12, {r12}",
            "mov r13, {r13}",
            "mov r14, {r14}",
            "mov r15, {r15}",
            "mov rbp, {rbp}",
            // iretq が CS/RIP/RFLAGS->(RSP/SS) を期待するので順に push
            "push {ss}",
            "push {user_rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            // restore user GS base from IA32_KERNEL_GS_BASE
            "swapgs",
            "iretq",
            rbx = in(reg) saved.rbx,
            r12 = in(reg) saved.r12,
            r13 = in(reg) saved.r13,
            r14 = in(reg) saved.r14,
            r15 = in(reg) saved.r15,
            rbp = in(reg) saved.rbp,
            ss = in(reg) user_ds as u64,
            user_rsp = in(reg) saved.rsp,
            rflags = in(reg) saved.rflags,
            cs = in(reg) user_cs,
            rip = in(reg) saved.rip,
            options(noreturn)
        );
    }
    crate::debug!("  End of switch_to_thread");
}
