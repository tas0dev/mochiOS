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

use core::sync::atomic::{AtomicU64, Ordering};

/// 現在のスレッドのカーネルスタックトップ (SYSCALL 時に切り替える)
/// コンテキストスイッチ時に更新される
pub static SYSCALL_KERNEL_RSP: AtomicU64 = AtomicU64::new(0);

/// SYSCALL 入口でユーザー RSP を一時退避する領域 (シングルCPU用)
static SYSCALL_TEMP_USER_RSP: AtomicU64 = AtomicU64::new(0);

/// SYSCALL 入口でユーザー FS ベースを一時退避する領域
static SYSCALL_TEMP_USER_FSBASE: AtomicU64 = AtomicU64::new(0);

/// SYSCALL 用カーネルスタック (初回スイッチまたはスレッド切り替え前に使用)
#[repr(align(16))]
struct SyscallStack([u8; 4096 * 8]);
static mut SYSCALL_KERNEL_STACK: SyscallStack = SyscallStack([0; 4096 * 8]);

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

    // 初期カーネルスタックを設定
    let kstack_top = unsafe {
        let base = SYSCALL_KERNEL_STACK.0.as_ptr() as u64;
        base + 4096 * 8
    };
    SYSCALL_KERNEL_RSP.store(kstack_top, Ordering::Relaxed);

    crate::info!("SYSCALL/SYSRET initialized: LSTAR={:#x}", lstar_val);
}

/// SYSCALL カーネルスタックを更新する (コンテキストスイッチ時に呼ぶ)
pub fn update_kernel_rsp(rsp: u64) {
    SYSCALL_KERNEL_RSP.store(rsp, Ordering::Relaxed);
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
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        // ユーザー RSP を一時退避 (グローバル変数: シングルCPU前提)
        "mov [{temp_rsp}], rsp",

        // ユーザー FS ベースを IA32_FS_BASE MSR から読み込んで一時退避
        // (RDFSBASE は FSGSBASE 未対応CPUで #UD になるため MSR を使用)
        "mov ecx, 0xC0000100",  // IA32_FS_BASE MSR
        "rdmsr",
        "shl rdx, 32",
        "or rdx, rax",
        "mov [{temp_fsbase}], rdx",

        // カーネルスタックに切り替え
        "mov rsp, [{kernel_rsp}]",

        // カーネルスタック上にコンテキストを保存
        // SYSRETQ に必要: RCX (user RIP), R11 (user RFLAGS), user RSP
        "push rcx",                         // user RIP
        "push r11",                         // user RFLAGS
        "push [{temp_rsp}]",                // user RSP (直接 push できないので一旦 rax 経由)

        // Callee-saved レジスタ保存
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // カーネルデータセグメントを設定
        "mov cx, 0x10",
        "mov ds, cx",
        "mov es, cx",

        // 割り込みを再有効化 (カーネルスタックに切り替え済みなので安全)
        "sti",

        // syscall 引数を System V ABI に並べ替えて dispatch を呼ぶ
        // dispatch(num, arg0, arg1, arg2, arg3, arg4)
        // SysV:   rdi,  rsi,  rdx,  rcx,  r8,  r9
        // 入力:   rax,  rdi,  rsi,  rdx,  r10, r8
        "mov r9,  r8",          // arg4 → r9
        "mov r8,  r10",         // arg3 → r8  (Linux: arg3 は r10)
        "mov rcx, rdx",         // arg2 → rcx
        "mov rdx, rsi",         // arg1 → rdx
        "mov rsi, rdi",         // arg0 → rsi
        "mov rdi, rax",         // num  → rdi
        "call {dispatch}",

        // 割り込みを禁止 (ユーザーコンテキスト復元前)
        "cli",

        // Callee-saved レジスタ復元
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",

        // ユーザー FS ベースを IA32_FS_BASE MSR に復元 (TLS)
        // rax/rdx/ecx はここで自由に使える (user RSP/RIP はまだスタックに積んである)
        "mov rax, [{temp_fsbase}]",
        "mov rdx, rax",
        "shr rdx, 32",
        "mov ecx, 0xC0000100",  // IA32_FS_BASE MSR
        "wrmsr",

        // ユーザーコンテキスト復元 (SYSRETQ に必要: rcx=RIP, r11=RFLAGS, rsp=RSP)
        "pop rdx",   // user RSP を一時 rdx に
        "pop r11",   // user RFLAGS
        "pop rcx",   // user RIP

        // ユーザーデータセグメントを設定 (ax は自由なので使用)
        "mov ax, 0x1b",
        "mov ds, ax",
        "mov es, ax",

        // ユーザー RSP に切り替えて SYSRETQ
        "mov rsp, rdx",
        "sysretq",

        temp_rsp   = sym SYSCALL_TEMP_USER_RSP,
        temp_fsbase = sym SYSCALL_TEMP_USER_FSBASE,
        kernel_rsp = sym SYSCALL_KERNEL_RSP,
        dispatch   = sym super::syscall_dispatch_sysv,
    );
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
