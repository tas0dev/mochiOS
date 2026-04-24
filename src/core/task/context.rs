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

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
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
///
/// # Safety
/// `old_context`/`new_context` は有効な `Context` 領域を指し、呼び出し規約に従って
/// コンテキスト切替可能な状態である必要がある。
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn switch_context(old_context: *mut Context, new_context: *const Context) {
    core::arch::naked_asm!(
        "cli",
        // save current (ret address is at [rsp])
        "lea rax, [rsp + 0x08]",
        // System V AMD64 ABI (used by x86_64-unknown-none):
        // 第1引数 (old_context) = rdi
        // 第2引数 (new_context) = rsi
        "mov [rdi + 0x00], rax", // rsp
        "mov [rdi + 0x08], rbp", // rbp
        "mov [rdi + 0x10], rbx", // rbx
        "mov [rdi + 0x18], r12", // r12
        "mov [rdi + 0x20], r13", // r13
        "mov [rdi + 0x28], r14", // r14
        "mov [rdi + 0x30], r15", // r15
        "mov [rdi + 0x38], rdi", // rdi
        "mov [rdi + 0x40], rsi", // rsi
        // 戻り先アドレス（call命令でスタックにpushされている）を保存
        "mov rax, [rsp]",
        "mov [rdi + 0x48], rax", // rip
        // RFLAGSを保存
        "pushfq",
        "pop rax",
        "mov [rdi + 0x50], rax", // rflags
        // 新しいコンテキストを復元
        "mov rax, [rsi + 0x48]", // 新しいrip
        "mov r11, [rsi + 0x50]", // 新しいrflags
        "mov rbx, [rsi + 0x10]", // rbx
        "mov r12, [rsi + 0x18]", // r12
        "mov r13, [rsi + 0x20]", // r13
        "mov r14, [rsi + 0x28]", // r14
        "mov r15, [rsi + 0x30]", // r15
        "mov rdi, [rsi + 0x38]", // rdi
        "mov rbp, [rsi + 0x08]", // rbp
        "mov rsp, [rsi + 0x00]", // rsp (rsiを最後に使う)
        "mov rsi, [rsi + 0x40]", // rsi (rspセット後、rsiを上書きしてOK)
        // RFLAGSを復元
        "push r11",
        "popfq",
        "jmp rax",
    );
}

/// 別スレッドへ切替（通常呼び出し経路）
///
/// # Safety
/// 呼び出し側は `next_id` が有効な実行可能スレッドであることを保証する必要がある。
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

    let (old_ctx_ptr, current_process_id, current_priv) = if let Some(id) = current_id {
        if let Some(thread) = queue.get_mut(id) {
            if !thread.is_kernel_stack_guard_intact() {
                let bottom = thread.kernel_stack_bottom();
                let top = thread.kernel_stack_top();
                drop(queue);
                crate::error!(
                    "Kernel stack guard corrupted: tid={:?}, kstack=[{:#x}..{:#x})",
                    id,
                    bottom,
                    top
                );
                crate::audit::log(
                    crate::audit::AuditEventKind::Quarantine,
                    "context switch detected kernel stack corruption",
                );
                crate::task::terminate_thread(id);
                return;
            }
            let ptr = thread.context_mut() as *mut Context;
            crate::debug!(
                "  Current context ptr: {:p}, rsp={:#x}, rip={:#x}",
                ptr,
                thread.context().rsp,
                thread.context().rip
            );
            let pid = thread.process_id();
            let priv_level =
                with_process(pid, |p| p.privilege()).unwrap_or(crate::task::PrivilegeLevel::Core);
            (ptr, Some(pid), priv_level)
        } else {
            return; // 現在のスレッドが見つからない
        }
    } else {
        // 現在のスレッドがない場合（初回スイッチ）はダミーに書き込む（値は捨てられる）
        crate::debug!("  No current thread (initial switch)");
        (
            unsafe { core::ptr::addr_of_mut!(INITIAL_DUMMY_CONTEXT) },
            None,
            crate::task::PrivilegeLevel::Core,
        )
    };

    // 次のスレッドのコンテキストへのポインタとカーネルスタックトップを取得
    let (
        new_context_ptr,
        next_kstack_top,
        next_process_id,
        next_fs_base,
        next_in_syscall,
        next_priv,
    ) = if let Some(thread) = queue.get(next_id) {
        let ptr = thread.context() as *const Context;
        let kstack = thread.kernel_stack_top();
        let pid = thread.process_id();
        let fs = thread.fs_base();
        let in_syscall = thread.in_syscall();
        let priv_level =
            with_process(pid, |p| p.privilege()).unwrap_or(crate::task::PrivilegeLevel::Core);
        crate::debug!(
            "  Next context ptr: {:p}, rsp={:#x}, rip={:#x}, kstack={:#x}",
            ptr,
            thread.context().rsp,
            thread.context().rip,
            kstack
        );
        (ptr, kstack, pid, fs, in_syscall, priv_level)
    } else {
        return; // 次のスレッドが見つからない
    };

    drop(queue);

    // 実際に切り替える直前に current thread を更新する。
    // これにより「currentだけ先に更新される競合窓」を避ける。
    crate::task::set_current_thread(Some(next_id));

    // TSSのRSP0とSYSCALL用カーネルスタックを更新
    crate::mem::tss::set_rsp0(next_kstack_top);
    crate::syscall::syscall_entry::update_kernel_rsp(next_kstack_top);
    // SYSCALL 入口の swapgs により IA32_KERNEL_GS_BASE がユーザー値へ一時退避されるため、
    // ブロッキング syscall 中に他スレッドへ切り替える前に per-CPU GS ベースへ戻しておく。
    crate::percpu::install_current_cpu_gs_base();
    let predictor_domain_changed =
        current_process_id != Some(next_process_id) || current_priv != next_priv;
    if predictor_domain_changed {
        crate::cpu::branch_predictor_barrier();
    }
    crate::cpu::reassert_runtime_hardening();

    // 次のスレッドの FS ベースを復元 (TLS)
    unsafe {
        crate::cpu::write_fs_base(next_fs_base);
    }

    // 次のコンテキストがカーネル実行の場合はカーネルCR3に固定する
    if next_priv == crate::task::PrivilegeLevel::Core || next_in_syscall {
        let kernel_cr3 = crate::percpu::kernel_cr3();
        if kernel_cr3 != 0 {
            crate::mem::paging::switch_page_table(kernel_cr3);
        }
    } else if let Some(pt_phys) = with_process(next_process_id, |p| p.page_table()).flatten() {
        crate::mem::paging::switch_page_table(pt_phys);
    }

    crate::debug!("About to perform context switch...");
    switch_context(old_ctx_ptr, new_context_ptr);
}

/// カーネルから直接ユーザーモードに入るためのヘルパ（最初のユーザスレッド用）
///
/// # Safety
/// `ctx` はユーザーモード復帰に必要な有効なレジスタ値/セグメント値を含んでいる必要がある。
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
        ss = in(reg) user_ds,
        user_rsp = in(reg) ctx.rsp,
        rflags = in(reg) ctx.rflags,
        cs = in(reg) user_cs,
        rip = in(reg) ctx.rip,
        options(noreturn)
    )
}

/// 割込み内からの切替。呼び出し側で割込み時のレジスタを `saved` に収めて渡す。
///
/// # Safety
/// `saved` は現在スレッドの正しい保存コンテキストであり、`next_id` は有効な
/// 実行可能スレッドである必要がある。
pub unsafe fn switch_to_thread_from_isr(
    current_id: Option<ThreadId>,
    next_id: ThreadId,
    saved: Context,
) {
    let mut queue = THREAD_QUEUE.lock();

    let (old_ctx_ptr, current_process_id, current_priv) = if let Some(id) = current_id {
        if let Some(thread) = queue.get_mut(id) {
            if !thread.is_kernel_stack_guard_intact() {
                let bottom = thread.kernel_stack_bottom();
                let top = thread.kernel_stack_top();
                drop(queue);
                crate::error!(
                    "Kernel stack guard corrupted (ISR): tid={:?}, kstack=[{:#x}..{:#x})",
                    id,
                    bottom,
                    top
                );
                crate::audit::log(
                    crate::audit::AuditEventKind::Quarantine,
                    "isr context switch detected kernel stack corruption",
                );
                crate::task::terminate_thread(id);
                return;
            }
            let pid = thread.process_id();
            let priv_level =
                with_process(pid, |p| p.privilege()).unwrap_or(crate::task::PrivilegeLevel::Core);
            (thread.context_mut() as *mut Context, Some(pid), priv_level)
        } else {
            return;
        }
    } else {
        (
            unsafe { core::ptr::addr_of_mut!(INITIAL_DUMMY_CONTEXT) },
            None,
            crate::task::PrivilegeLevel::Core,
        )
    };

    let (new_ctx_ptr, next_priv, next_kstack_top, next_fs_base, next_process_id, next_in_syscall) =
        if let Some(thread) = queue.get(next_id) {
            let ptr = thread.context() as *const Context;
            let proc = thread.process_id();
            let priv_level =
                with_process(proc, |p| p.privilege()).unwrap_or(crate::task::PrivilegeLevel::Core);
            let kstack = thread.kernel_stack_top();
            let fs = thread.fs_base();
            let in_syscall = thread.in_syscall();
            (ptr, priv_level, kstack, fs, proc, in_syscall)
        } else {
            return;
        };

    if !old_ctx_ptr.is_null() {
        unsafe {
            *old_ctx_ptr = saved;
        }
    }

    drop(queue);

    // ISR 経路でも、実際の遷移直前に current thread を更新する。
    crate::task::set_current_thread(Some(next_id));

    // TSSのRSP0を更新
    crate::mem::tss::set_rsp0(next_kstack_top);

    // SYSCALL用カーネルスタックも更新 (次のスレッドのカーネルスタックを使う)
    crate::syscall::syscall_entry::update_kernel_rsp(next_kstack_top);
    // SYSCALL 入口の swapgs により IA32_KERNEL_GS_BASE がユーザー値へ一時退避されるため、
    // ブロッキング syscall 中に他スレッドへ切り替える前に per-CPU GS ベースへ戻しておく。
    crate::percpu::install_current_cpu_gs_base();
    let predictor_domain_changed =
        current_process_id != Some(next_process_id) || current_priv != next_priv;
    if predictor_domain_changed {
        crate::cpu::branch_predictor_barrier();
    }
    crate::cpu::reassert_runtime_hardening();

    // 次のスレッドの FS ベースを復元 (TLS)
    crate::cpu::write_fs_base(next_fs_base);

    // 次のコンテキストがカーネル実行の場合はカーネルCR3に固定する
    if next_priv == crate::task::PrivilegeLevel::Core || next_in_syscall {
        let kernel_cr3 = crate::percpu::kernel_cr3();
        if kernel_cr3 != 0 {
            crate::mem::paging::switch_page_table(kernel_cr3);
        }
    } else if let Some(pt_phys) = with_process(next_process_id, |p| p.page_table()).flatten() {
        crate::mem::paging::switch_page_table(pt_phys);
    }

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
            ss = in(reg) user_ds,
            user_rsp = in(reg) saved.rsp,
            rflags = in(reg) saved.rflags,
            cs = in(reg) user_cs,
            rip = in(reg) saved.rip,
            options(noreturn)
        );
    }
}
