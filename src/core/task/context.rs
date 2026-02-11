use crate::task::ids::ThreadId;
use crate::task::thread::THREAD_QUEUE;

/// CPUコンテキスト（レジスタ保存用）
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Context {
    /// スタックポインタ
    pub rsp: u64,
    /// ベースポインタ
    pub rbp: u64,
    /// Callee-saved レジスタ
    pub rbx: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rdi: u64, // MS ABI Callee-saved
    pub rsi: u64, // MS ABI Callee-saved
    /// 命令ポインタ（戻り先アドレス）
    pub rip: u64,
    /// RFLAGSレジスタ
    pub rflags: u64,
}

impl Context {
    /// 新しいコンテキストを作成
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

/// コンテキストスイッチ
///
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switch_context(old_context: *mut Context, new_context: *const Context) {
    core::arch::naked_asm!(
        // コンテキストスイッチ中の割り込みを禁止
        "cli",
        // 現在のコンテキストを保存
        // 呼び出し元に戻った後の rsp を保存（ret 相当）
        "lea rax, [rsp + 0x08]",
        // System V AMD64 ABI (Rust default on x86_64-unknown-none):
        // 第1引数 (old_context) = rdi
        // 第2引数 (new_context) = rsi
        "mov [rdi + 0x00], rax", // rsp
        "mov [rdi + 0x08], rbp", // rbp
        "mov [rdi + 0x10], rbx", // rbx
        "mov [rdi + 0x18], r12", // r12
        "mov [rdi + 0x20], r13", // r13
        "mov [rdi + 0x28], r14", // r14
        "mov [rdi + 0x30], r15", // r15
        "mov [rdi + 0x38], rdi", // rdi (保存)
        "mov [rdi + 0x40], rsi", // rsi (保存)
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
        "mov rdi, [rsi + 0x38]", // rdi (復元)
        // rsi needs to be restored LAST because it holds the pointer to new_context
        // But we need to restore rsi from [rsi + 0x40].
        // So we load it into a temp register (rax is free now since we used it for rip, waits no we jump to rax)
        // We can use rbp or something? No, rbp restored.
        // We can use the STACK or a temp register?
        // r11 holds rflags, we push it after.
        // Let's use rax for rsi_value.
        "mov rax, [rsi + 0x40]", // rsi (value to restore)
        "mov rsi, rax",           // rsi restored (now rsi pointer is lost, but we don't need it anymore except for next fields... wait)

        // Wait, we need [rsi + 0x48] (RIP) and [rsi + 0x00] (RSP) and [rsi + 0x08] (RBP).
        // I restored rbp from [rsi + 0x08] ALREADY?
        // Let's reorder.

        // 1. Load everything we need from [rsi] while [rsi] is still valid.
        "mov rbp, [rsi + 0x08]", // rbp
        "mov rsp, [rsi + 0x00]", // rsp
        "mov rax, [rsi + 0x48]", // rip (target)

        // 2. Restore GPRs
        // rbx, r12, r13, r14, r15 done above or here.
        // rdi restored above or here.
        // rsi restored LAST.

        // Let's rewrite the restore part clearly
        "mov rbx, [rsi + 0x10]", // rbx
        "mov r12, [rsi + 0x18]", // r12
        "mov r13, [rsi + 0x20]", // r13
        "mov r14, [rsi + 0x28]", // r14
        "mov r15, [rsi + 0x30]", // r15
        "mov rdi, [rsi + 0x38]", // rdi
        // Now carefully restore rsi. We need rsi pointer to read rsi value.
        // But we also need 'rax' (rip) and 'r11' (rflags) preserved.
        // We can push rsi value to stack? But we just switched stack!
        // Yes, we switched RSP to new stack. We can push to new stack?
        // But we assume the stack is clean/prepared.
        // We can just use a register that we haven't restored yet, or overwrite one temp.
        // We haven't restored 'rsi' yet.
        // We can use 'rcx' or 'rdx' as scratch! (Caller saved).
        "mov rcx, [rsi + 0x40]", // load new rsi value into rcx
        "mov rsi, rcx",           // restore rsi

        // RFLAGSを復元
        "push r11",
        "popfq",
        // 新しいripへジャンプ
        "jmp rax"
    );
}

/// 現在のスレッドから指定されたスレッドIDにコンテキストスイッチ
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

    // 現在のスレッドのコンテキストへのポインタを取得
    let old_context_ptr = if let Some(id) = current_id {
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
        // 現在のスレッドがない場合（初回スイッチ）
        // ダミーのコンテキストを使用
        crate::debug!("  No current thread (initial switch)");
        core::ptr::null_mut()
    };

    // 次のスレッドのコンテキストへのポインタを取得
    let new_context_ptr = if let Some(thread) = queue.get(next_id) {
        let ptr = thread.context() as *const Context;
        crate::debug!(
            "  Next context ptr: {:p}, rsp={:#x}, rip={:#x}",
            ptr,
            thread.context().rsp,
            thread.context().rip
        );
        ptr
    } else {
        return; // 次のスレッドが見つからない
    };

    // ロックを解放してからコンテキストスイッチ
    drop(queue);

    crate::debug!("About to perform context switch...");

    // コンテキストスイッチを実行
    if old_context_ptr.is_null() {
        // 初回スイッチの場合、現在のコンテキストを保存せずにジャンプ
        crate::debug!("Initial context switch (no save)");
        let ctx = &*new_context_ptr;
        core::arch::asm!(
            "cli",
            "mov rsp, rax",       // rsp = ctx.rsp
            "mov rbp, {rbp_val}", // Restore rbp
            "mov rbx, {rbx_val}", // Restore rbx
            "push rcx",           // push rflags
            "popfq",              // restore rflags
            "jmp rdx",            // jump to rip

            // Fixed registers
            in("rax") ctx.rsp,
            in("rcx") ctx.rflags,
            in("rdx") ctx.rip,

            in("r12") ctx.r12,
            in("r13") ctx.r13,
            in("r14") ctx.r14,
            in("r15") ctx.r15,
            in("rdi") ctx.rdi,
            in("rsi") ctx.rsi,

            // Compiler allocated registers (will use r8-r11)
            rbp_val = in(reg) ctx.rbp,
            rbx_val = in(reg) ctx.rbx,

            options(noreturn)
        );
    } else {
        crate::debug!("Normal context switch (save and restore)");
        crate::debug!(
            "  Calling switch_context({:p}, {:p})",
            old_context_ptr,
            new_context_ptr
        );
        switch_context(old_context_ptr, new_context_ptr);
        crate::debug!("  Returned from switch_context");
    }
    crate::debug!("  End of switch_to_thread");
}
