//! SYSCALL/SYSRET 命令サポート
//!
//! Linux x86_64 ABI: syscall 命令を使ったシステムコールをサポートする。
//! int 0x80 との違いは SYSCALL 専用の MSR 設定が必要な点。
//!
//! SYSCALL 時のレジスタ:
//!   RAX = syscall 番号
//!   RDI = arg0, RSI = arg1, RDX = arg2, R10 = arg3, R8 = arg4, R9 = arg5
//!   RCX = ユーザーの RIP (SYSCALL が自動保存)
//!   R11 = ユーザーの RFLAGS (SYSCALL が自動保存)
//!   RSP = まだユーザースタック

/// SYSCALL/SYSRET に必要な MSR を初期化する
///
/// カーネル GDT 構成 (x86_64-unknown-uefi / MS ABI):
///   index 0: null
///   index 1: kernel code (CS)   → selector = 0x08
///   index 2: kernel data (SS)   → selector = 0x10
///   index 3: user data (SS)     → selector = 0x18
///   index 4: user code (CS)     → selector = 0x20
///   index 5: TSS (2 entries)
///
/// STAR MSR レイアウト:
///   [47:32] = SYSCALL CS (カーネル CS, SS は CS+8)
///   [63:48] = SYSRET CS  (ユーザー CS-16, 実際の user CS は +16, SS は +8)
pub fn init_syscall() {
    // IA32_EFER の SCE ビットを有効化 (SYSCALL/SYSRET を使えるようにする)
    const IA32_EFER: u32 = 0xC000_0080;
    const SCE_BIT: u64 = 1;

    // IA32_STAR: [47:32] = kernel CS selector, [63:48] = user CS selector - 16
    // カーネル: CS=0x08, SS=0x10
    // ユーザー: CS=0x23 (0x20|3), SS=0x1b (0x18|3)
    // STAR[47:32] = 0x0008 (kernel CS)
    // STAR[63:48] = 0x0010 (= user_cs - 16 で SYSRET時に +16 される → 0x20, RPL=3 付与で 0x23)
    const IA32_STAR: u32 = 0xC000_0081;
    let star_val: u64 = ((0x0008u64) << 32) | ((0x0010u64) << 48);

    // IA32_LSTAR: SYSCALL 時のエントリポイント (64ビット)
    const IA32_LSTAR: u32 = 0xC000_0082;
    let lstar_val = syscall_entry as *const () as u64;

    // IA32_FMASK: SYSCALL 時に RFLAGS からクリアするビット
    // IF (bit 9) をクリアして割り込み禁止にする
    const IA32_FMASK: u32 = 0xC000_0084;
    let fmask_val: u64 = 0x200; // IF ビット

    unsafe {
        // EFER.SCE を設定
        let efer = read_msr(IA32_EFER);
        write_msr(IA32_EFER, efer | SCE_BIT);

        write_msr(IA32_STAR, star_val);
        write_msr(IA32_LSTAR, lstar_val);
        write_msr(IA32_FMASK, fmask_val);
    }

    // 初期カーネルスタックは現在のRSPを使用し、後続のコンテキストスイッチで更新する
    let kstack_top: u64;
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) kstack_top, options(nomem, nostack, preserves_flags));
    }
    crate::percpu::init_boot_cpu(kstack_top);

    crate::info!("SYSCALL/SYSRET initialized: LSTAR={:#x}", lstar_val);
}

/// SYSCALL カーネルスタックを更新する (コンテキストスイッチ時に呼ぶ)
pub fn update_kernel_rsp(rsp: u64) {
    // SeqCst を使用してメモリ順序を保証する (MED-05)
    crate::percpu::set_syscall_kernel_rsp(rsp);
}

/// KPTI: 現在CR3がユーザーならカーネルCR3へ切り替え、元のCR3を返す
pub fn switch_to_kernel_page_table() -> u64 {
    let kernel_cr3 = crate::percpu::kernel_cr3();
    if kernel_cr3 == 0 {
        return 0;
    }
    let (current_cr3, _) = x86_64::registers::control::Cr3::read();
    let current = current_cr3.start_address().as_u64();
    if current == kernel_cr3 {
        return 0;
    }
    crate::mem::paging::switch_page_table(kernel_cr3);
    current
}

/// KPTI: 以前のCR3へ戻す（0はno-op）
pub fn restore_page_table(previous_cr3: u64) {
    if previous_cr3 != 0 {
        crate::mem::paging::switch_page_table(previous_cr3);
    }
}

/// KPTI: 現在スレッドがユーザー権限なら、そのプロセスのユーザーCR3へ切り替える
pub fn switch_to_current_thread_user_page_table() {
    let tid = match crate::task::current_thread_id() {
        Some(t) => t,
        None => return,
    };
    let pid = match crate::task::with_thread(tid, |t| t.process_id()) {
        Some(p) => p,
        None => return,
    };
    let is_core = crate::task::with_process(pid, |p| p.privilege())
        .is_some_and(|lvl| lvl == crate::task::PrivilegeLevel::Core);
    if is_core {
        return;
    }
    if let Some(user_pt) = crate::task::with_process(pid, |p| p.page_table()).flatten() {
        crate::mem::paging::switch_page_table(user_pt);
    }
}

/// KPTI: SYSCALL/INT入口でカーネルCR3へ切り替える
pub fn kpti_enter_for_current_thread() {
    let previous = switch_to_kernel_page_table();
    if let Some(tid) = crate::task::current_thread_id() {
        crate::task::with_thread_mut(tid, |t| t.set_syscall_user_cr3(previous));
    }
}

/// KPTI: SYSCALL/INT出口でユーザーCR3へ戻す
pub fn kpti_leave_for_current_thread() {
    let restore = crate::task::current_thread_id()
        .and_then(|tid| {
            crate::task::with_thread_mut(tid, |t| {
                let cr3 = t.syscall_user_cr3();
                t.set_syscall_user_cr3(0);
                cr3
            })
        })
        .unwrap_or(0);
    restore_page_table(restore);
}

/// 例外/IRQ 入口で、ユーザー文脈から入った場合のみ kernel CR3 と hardening 状態をそろえる。
pub fn kpti_enter_for_trap(from_user: bool) -> bool {
    if !from_user {
        return false;
    }
    let previous = switch_to_kernel_page_table();
    if previous != 0 {
        crate::cpu::reassert_runtime_hardening();
    }
    previous != 0
}

/// 例外/IRQ からの復帰で、ユーザー文脈へ戻る場合は復帰先スレッドの user CR3 を再設定する。
pub fn kpti_leave_after_trap(entered_from_user: bool) {
    if entered_from_user {
        switch_to_current_thread_user_page_table();
    }
}

/// SYSCALL エントリポイント (naked function)
///
/// 呼ばれた時点:
///   RSP = ユーザースタック (そのまま)
///   RCX = ユーザー RIP
///   R11 = ユーザー RFLAGS
///   RAX = syscall 番号
///   RDI/RSI/RDX/R10/R8/R9 = 引数
///   割り込み: 禁止 (FMASK で IF クリア済み)
///
/// # Safety
/// CPU が SYSCALL エントリ規約どおりのレジスタ状態でこの関数へ入ることを前提とする。
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        // ユーザーRSPを per-CPU 一時領域へ退避してからカーネルスタックへ切り替える
        "swapgs",
        "mov qword ptr gs:[{user_rsp_tmp_off}], rsp",
        "mov rsp, qword ptr gs:[{sys_rsp_off}]",
        // ユーザー復帰時に必要な GPR を保存（Linux syscall ABI: RCX/R11以外も保持）
        "push rax",                             // syscall 番号
        "push rdi",                             // arg0
        "push rsi",                             // arg1
        "push rdx",                             // arg2
        "push r10",                             // arg3
        "push r8",                              // arg4
        "push r9",                              // arg5
        "push rcx",                             // user RIP
        "push r11",                             // user RFLAGS
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "push qword ptr gs:[{user_rsp_tmp_off}]", // user RSP
        // call 前の 16-byte alignment を満たす
        "sub rsp, 8",

        // カーネルデータセグメントを設定
        "mov cx, 0x10",
        "mov ds, cx",
        "mov es, cx",

        // fork/clone のときだけ現在スレッドへユーザーコンテキストを記録
        // align slot ありレイアウト:
        // [rsp+8]=user RSP, [rsp+64]=user RFLAGS, [rsp+72]=user RIP, [rsp+128]=syscall num
        "mov rax, [rsp + 128]",
        "cmp rax, 56",
        "je 3f",
        "cmp rax, 57",
        "jne 4f",
        "3:",
        "mov rdi, rax",
        "mov rsi, [rsp + 72]",  // user RIP
        "mov rdx, [rsp + 8]",   // user RSP
        "mov rcx, [rsp + 64]",  // user RFLAGS
        "call {save_ctx_fn}",
        "4:",

        // 割り込みを再有効化 (カーネルスタックに切り替え済みなので安全)
        "sti",

        // syscall 引数を System V ABI に並べ替えて dispatch を呼ぶ
        // dispatch(num, arg0, arg1, arg2, arg3, arg4)
        // align slot ありレイアウト:
        // [rsp+128]=num, [rsp+120]=arg0, [rsp+112]=arg1, [rsp+104]=arg2, [rsp+96]=arg3, [rsp+88]=arg4
        "mov rdi, [rsp + 128]",
        "mov rsi, [rsp + 120]",
        "mov rdx, [rsp + 112]",
        "mov rcx, [rsp + 104]",
        "mov r8,  [rsp + 96]",
        "mov r9,  [rsp + 88]",
        "call {dispatch}",

        // 割り込みを禁止 (ユーザーコンテキスト復元前)
        "cli",
        // 戻り値を保存スロットへ退避（元 num スロットを再利用）
        "mov [rsp + 128], rax",
        // align slot を捨てる
        "add rsp, 8",

        // 保存したユーザーコンテキストを復元
        "pop rax",              // user RSP（一時）
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "pop r11",              // user RFLAGS (SYSRET 用)
        "pop rcx",              // user RIP (SYSRET 用)
        "mov qword ptr gs:[{user_rsp_tmp_off}], rax",

        // ユーザー FS ベースを現在スレッド状態から再取得して復元 (TLS)
        "push rcx",
        "push r11",
        "push rax",
        "call {fs_base_fn}",
        "mov r8, rax",
        "mov rdx, rax",
        "shr rdx, 32",
        "mov ecx, 0xC0000100",  // IA32_FS_BASE MSR (ecx を上書きしても安全)
        "mov rax, r8",
        "wrmsr",
        "pop rax",
        "pop r11",
        "pop rcx",

        // CVE-2012-0217 緩和策: SYSRETQ 前にユーザー RIP/RSP の正規アドレスチェック
        // Intel CPU では SYSRETQ 実行時にRCX/RSPが非正規アドレス（bit 63:47 が不一致）だと
        // Ring 0 で #GP が発生し、攻撃者が制御フローを握る恐れがある (CVE-2012-0217)
        // ユーザー空間の正規アドレス: bit 63:47 = 0b000...0 (0x0000_7FFF_FFFF_FFFF 以下)
        "mov rax, qword ptr gs:[{user_rsp_tmp_off}]",
        "sar rax, 47",          // 算術右シフト47bit: 正規なら全ビット0
        "test rax, rax",
        "jnz 2f",               // 非正規アドレス → プロセスを終了
        "mov rdx, rcx",
        "sar rdx, 47",
        "test rdx, rdx",
        "jnz 2f",

        // ユーザーデータセグメントを再設定（後で rdx は復元される）
        "mov dx, 0x1b",
        "mov ds, dx",
        "mov es, dx",

        // Linux syscall ABI に合わせて volatile 引数レジスタも復元
        "pop r9",
        "pop r8",
        "pop r10",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop rax",              // syscall 戻り値

        // ユーザー RSP に切り替えて SYSRETQ
        "mov rsp, qword ptr gs:[{user_rsp_tmp_off}]",
        "swapgs",
        "sysretq",

        // 非正規RIP/RSP検出: カーネルスタックに戻してプロセスを終了
        "2:",
        "mov rsp, qword ptr gs:[{sys_rsp_off}]",
        "call {kill_fn}",

        sys_rsp_off = const crate::percpu::GS_SYSCALL_KERNEL_RSP_OFFSET,
        user_rsp_tmp_off = const crate::percpu::GS_SYSCALL_USER_RSP_TMP_OFFSET,
        save_ctx_fn = sym super::save_user_context_for_fork,
        fs_base_fn = sym current_thread_fs_base_for_sysret,
        dispatch   = sym super::syscall_dispatch_sysv,
        kill_fn    = sym kill_non_canonical_rsp,
    );
}

extern "sysv64" fn current_thread_fs_base_for_sysret() -> u64 {
    if let Some(tid) = crate::task::current_thread_id() {
        if let Some(fs_base) = crate::task::with_thread(tid, |t| t.fs_base()) {
            return fs_base;
        }
    }
    unsafe { crate::cpu::read_fs_base() }
}

/// CVE-2012-0217 緩和策: 非正規RIP/RSPを持つプロセスを終了させる
unsafe extern "C" fn kill_non_canonical_rsp() -> ! {
    crate::warn!("CVE-2012-0217: non-canonical user RIP/RSP detected, killing process");
    crate::task::exit_current_task(u64::MAX)
}

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    ((hi as u64) << 32) | (lo as u64)
}

unsafe fn write_msr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack)
    );
}
