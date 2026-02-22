use crate::task::ids::ThreadId;
use crate::task::thread::THREAD_QUEUE;
use crate::task::process::with_process;

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

    // 次のスレッドのコンテキストへのポインタとカーネルスタックトップを取得
    let (new_context_ptr, next_kstack_top, next_process_id, next_fs_base) = if let Some(thread) = queue.get(next_id) {
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

    // ロックを解放してからコンテキストスイッチ
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
    if let Some(pt_phys) = crate::task::with_process(next_process_id, |p| p.page_table()).flatten() {
        crate::mem::paging::switch_page_table(pt_phys);
    }

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
