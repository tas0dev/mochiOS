use super::ids::{ThreadId, PrivilegeLevel};
use super::thread::THREAD_QUEUE;

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
    pub rip: u64,
    pub rflags: u64,
}

impl Context {
    pub const fn new() -> Self {
        Self { rsp: 0, rbp: 0, rbx: 0, r12: 0, r13: 0, r14: 0, r15: 0, rip: 0, rflags: 0 }
    }
}

#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn switch_context(old_context: *mut Context, new_context: *const Context) {
    core::arch::naked_asm!(
        "cli",
        // save current (ret address is at [rsp])
        "lea rax, [rsp + 0x08]",
        "mov [rcx + 0x00], rax",
        "mov [rcx + 0x08], rbp",
        "mov [rcx + 0x10], rbx",
        "mov [rcx + 0x18], r12",
        "mov [rcx + 0x20], r13",
        "mov [rcx + 0x28], r14",
        "mov [rcx + 0x30], r15",
        "mov rax, [rsp]",
        "mov [rcx + 0x38], rax",
        "pushfq",
        "pop rax",
        "mov [rcx + 0x40], rax",
        // restore new
        "mov rax, [rdx + 0x38]",
        "mov r11, [rdx + 0x40]",
        "mov rbx, [rdx + 0x10]",
        "mov r12, [rdx + 0x18]",
        "mov r13, [rdx + 0x20]",
        "mov r14, [rdx + 0x28]",
        "mov r15, [rdx + 0x30]",
        "mov rbp, [rdx + 0x08]",
        "mov rsp, [rdx + 0x00]",
        "push r11",
        "popfq",
        "jmp rax",
    );
}

/// 別スレッドへ切替（通常呼び出し経路）
pub unsafe fn switch_to_thread(current_id: Option<ThreadId>, next_id: ThreadId) {
    crate::info!("switch_to_thread: current={:?}, next={:?}", current_id, next_id);

    let mut queue = THREAD_QUEUE.lock();

    let old_ctx_ptr = if let Some(id) = current_id {
        if let Some(thread) = queue.get_mut(id) {
            thread.context_mut() as *mut Context
        } else { return; }
    } else { core::ptr::null_mut() };

    let new_ctx_ptr = if let Some(thread) = queue.get(next_id) {
        thread.context() as *const Context
    } else { return; };

    drop(queue);

    if old_ctx_ptr.is_null() {
        // 初回切替（保存先なし）
        let ctx = &*new_ctx_ptr;
        core::arch::asm!(
            "cli",
            "mov rsp, {rsp}",
            "mov rbp, {rbp}",
            "mov rbx, {rbx}",
            "mov r12, {r12}",
            "mov r13, {r13}",
            "mov r14, {r14}",
            "mov r15, {r15}",
            "push {rflags}",
            "popfq",
            "jmp {rip}",
            rsp = in(reg) ctx.rsp,
            rbp = in(reg) ctx.rbp,
            rbx = in(reg) ctx.rbx,
            r12 = in(reg) ctx.r12,
            r13 = in(reg) ctx.r13,
            r14 = in(reg) ctx.r14,
            r15 = in(reg) ctx.r15,
            rflags = in(reg) ctx.rflags,
            rip = in(reg) ctx.rip,
            options(noreturn)
        );
    } else {
        // 通常の保存->復元経路
        switch_context(old_ctx_ptr, new_ctx_ptr);
    }
}

/// 割込み内からの切替。呼び出し側で割込み時のレジスタを `saved` に収めて渡す。
pub unsafe fn switch_to_thread_from_isr(current_id: Option<ThreadId>, next_id: ThreadId, saved: Context) {
    crate::debug!("switch_to_thread_from_isr: current={:?}, next={:?}", current_id, next_id);

    let mut queue = THREAD_QUEUE.lock();

    let old_ctx_ptr = if let Some(id) = current_id {
        if let Some(thread) = queue.get_mut(id) { thread.context_mut() as *mut Context } else { return; }
    } else { core::ptr::null_mut() };

    let (new_ctx_ptr, next_priv) = if let Some(thread) = queue.get(next_id) {
        let ptr = thread.context() as *const Context;
        let proc = thread.process_id();
        let priv_level = crate::task::with_process(proc, |p| p.privilege()).unwrap_or(PrivilegeLevel::Core);
        (ptr, priv_level)
    } else { return; };

    if !old_ctx_ptr.is_null() { unsafe { *old_ctx_ptr = saved; } }

    drop(queue);

    let ctx = &*new_ctx_ptr;

    if next_priv == PrivilegeLevel::Core {
        core::arch::asm!(
            "cli",
            "mov rsp, {rsp}",
            "mov rbp, {rbp}",
            "mov rbx, {rbx}",
            "mov r12, {r12}",
            "mov r13, {r13}",
            "mov r14, {r14}",
            "mov r15, {r15}",
            "push {rflags}",
            "popfq",
            "jmp {rip}",
            rsp = in(reg) ctx.rsp,
            rbp = in(reg) ctx.rbp,
            rbx = in(reg) ctx.rbx,
            r12 = in(reg) ctx.r12,
            r13 = in(reg) ctx.r13,
            r14 = in(reg) ctx.r14,
            r15 = in(reg) ctx.r15,
            rflags = in(reg) ctx.rflags,
            rip = in(reg) ctx.rip,
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
            "mov ds, {ds:x}",
            "mov es, {ds:x}",
            "mov fs, {ds:x}",
            "mov gs, {ds:x}",
            // iretq が CS/RIP/RFLAGS->(RSP/SS) を期待するので順に push
            "push {ss}",
            "push {user_rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "iretq",
            rbx = in(reg) ctx.rbx,
            r12 = in(reg) ctx.r12,
            r13 = in(reg) ctx.r13,
            r14 = in(reg) ctx.r14,
            r15 = in(reg) ctx.r15,
            rbp = in(reg) ctx.rbp,
            ds = in(reg) user_ds as u64,
            ss = in(reg) user_ds as u64,
            user_rsp = in(reg) ctx.rsp,
            rflags = in(reg) ctx.rflags,
            cs = in(reg) user_cs,
            rip = in(reg) ctx.rip,
            options(noreturn)
        );
    }
}
