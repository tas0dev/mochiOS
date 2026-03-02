//! IDT (Interrupt Descriptor Table) 管理
//!
//! IDTの初期化と例外ハンドラの定義

use crate::{debug, error, mem::gdt, syscall, warn};
use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};
use x86_64::PrivilegeLevel;

static IDT: Once<InterruptDescriptorTable> = Once::new();

/// IDTを初期化
pub fn init() {
    debug!("Initializing IDT...");

    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        // CPU例外ハンドラ
        idt.divide_error.set_handler_fn(divide_error_handler);
        idt.debug.set_handler_fn(debug_handler);
        idt.non_maskable_interrupt.set_handler_fn(nmi_handler);
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.overflow.set_handler_fn(overflow_handler);
        idt.bound_range_exceeded
            .set_handler_fn(bound_range_exceeded_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.device_not_available
            .set_handler_fn(device_not_available_handler);

        // ダブルフォルトハンドラ（専用スタック使用）
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }

        idt.invalid_tss.set_handler_fn(invalid_tss_handler);
        idt.segment_not_present
            .set_handler_fn(segment_not_present_handler);
        idt.stack_segment_fault
            .set_handler_fn(stack_segment_fault_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.x87_floating_point
            .set_handler_fn(x87_floating_point_handler);
        idt.alignment_check.set_handler_fn(alignment_check_handler);
        idt.machine_check.set_handler_fn(machine_check_handler);
        idt.simd_floating_point
            .set_handler_fn(simd_floating_point_handler);
        idt.virtualization.set_handler_fn(virtualization_handler);

        // ハードウェア割り込みハンドラ（32-47番）
        idt[32].set_handler_fn(super::timer::timer_interrupt_handler); // Timer IRQ0
        idt[33].set_handler_fn(keyboard_interrupt_handler);             // Keyboard IRQ1 (C-2修正)

        // それ以外のハードウェア割り込みはとりあえずスタブ
        for i in 34..48 {
            idt[i].set_handler_fn(generic_interrupt_handler);
        }

        // システムコール割り込み (0x80)
        // naked functionなので、手動で設定
        unsafe {
            let handler_addr = syscall::syscall_interrupt_handler as *const () as u64;
            idt[0x80]
                .set_handler_addr(x86_64::VirtAddr::new(handler_addr))
                .set_privilege_level(PrivilegeLevel::Ring3);
        }

        // 48-255番も念のため設定（未使用の割り込みベクタ）
        for i in 48..=255 {
            if i == 0x80 {
                continue;
            }
            idt[i].set_handler_fn(generic_interrupt_handler);
        }

        idt
    });

    idt.load();

    // IDTが正しくロードされたか確認
    use x86_64::instructions::tables::sidt;
    let idtr = sidt();
    debug!(
        "IDT loaded: base={:p}, limit={}",
        idtr.base.as_ptr::<u8>(),
        idtr.limit
    );
}

/// CPU例外ハンドラ
///
/// 一般的なCPU例外（例: ゼロ除算、無効命令など）を処理するためのハンドラ
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    error!(
        "EXCEPTION: DIVIDE ERROR ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("{:#?}", stack_frame);
    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// デバッグ例外ハンドラ
///
/// デバッグ例外は、ブレークポイントやシングルステップなどのデバッグイベントで発生する
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn debug_handler(stack_frame: InterruptStackFrame) {
    debug!("EXCEPTION: DEBUG");
    debug!("{:#?}", stack_frame);
}

/// NMI (Non-Maskable Interrupt) ハンドラ
///
/// NMIはマスクできない割り込みで、通常はハードウェアの障害や緊急事態を知らせるために使用される
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn nmi_handler(stack_frame: InterruptStackFrame) {
    error!("EXCEPTION: NON-MASKABLE INTERRUPT");
    warn!("{:#?}", stack_frame);
    halt_cpu();
}

/// ブレークポイント例外ハンドラ
///
/// ブレークポイント例外は、INT3命令によって発生する。デバッグ目的で使用する
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    warn!("EXCEPTION: BREAKPOINT");
    debug!("{:#?}", stack_frame);
}

/// オーバーフロー例外ハンドラ
///
/// オーバーフロー例外は、INTO命令によって発生する。算術演算の結果がオーバーフローした場合に使用される
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn overflow_handler(stack_frame: InterruptStackFrame) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    error!(
        "EXCEPTION: OVERFLOW ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("{:#?}", stack_frame);
    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// BOUND RANGE EXCEEDED例外ハンドラ
///
/// BOUND RANGE EXCEEDED例外は、BOUND命令によって発生する。配列の範囲外アクセスなどで使用される
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn bound_range_exceeded_handler(stack_frame: InterruptStackFrame) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    error!(
        "EXCEPTION: BOUND RANGE EXCEEDED ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("{:#?}", stack_frame);
    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// 無効命令例外ハンドラ
///
/// 無効命令例外は、CPUが認識できない命令が実行されたときに発生する。ユーザーモードで発生した場合はプロセスを終了させ、カーネルモードで発生した場合はシステム全体を停止する
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    // ユーザーモードかチェック（code_segmentのRPLビットを確認）
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;

    error!(
        "EXCEPTION: INVALID OPCODE ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("{:#?}", stack_frame);

    if is_user_mode {
        use x86_64::registers::control::Cr3;
        use x86_64::structures::paging::{OffsetPageTable, PageTable, Translate};
        use x86_64::VirtAddr;

        if let Some(phys_off) = crate::mem::paging::physical_memory_offset() {
            let (frame, _) = Cr3::read();
            let l4_phys = frame.start_address().as_u64();
            let l4_ptr = (l4_phys + phys_off) as *mut PageTable;
            unsafe {
                let l4_ref = &mut *l4_ptr;
                let mut pt = OffsetPageTable::new(l4_ref, VirtAddr::new(phys_off));
                let rip = stack_frame.instruction_pointer.as_u64();

                // Instruction bytes dump
                let mut dump = [0u8; 16];
                for i in 0..dump.len() {
                    let va = VirtAddr::new(rip + i as u64);
                    if let Some(pa) = pt.translate_addr(va) {
                        let kaddr = (pa.as_u64() + phys_off) as *const u8;
                        dump[i] = core::ptr::read_volatile(kaddr);
                    } else {
                        dump[i] = 0xff;
                    }
                }
                error!("Instruction bytes @ {:#x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                    rip,
                    dump[0], dump[1], dump[2], dump[3],
                    dump[4], dump[5], dump[6], dump[7],
                    dump[8], dump[9], dump[10], dump[11],
                    dump[12], dump[13], dump[14], dump[15],
                );

                // Page table entries (L4..L1)
                let va = VirtAddr::new(rip);
                let l4_idx = ((va.as_u64() >> 39) & 0x1ff) as usize;
                let l3_idx = ((va.as_u64() >> 30) & 0x1ff) as usize;
                let l2_idx = ((va.as_u64() >> 21) & 0x1ff) as usize;
                let l1_idx = ((va.as_u64() >> 12) & 0x1ff) as usize;

                let l4 = &mut *l4_ptr;
                let e4 = &l4[l4_idx];
                if e4.is_unused() {
                    error!("P4 entry {} is unused", l4_idx);
                } else {
                    error!(
                        "P4 entry {}: addr={:#x}, flags={:?}",
                        l4_idx,
                        e4.addr().as_u64(),
                        e4.flags()
                    );
                    let l3_phys = e4.addr().as_u64();
                    let l3 = &*((l3_phys + phys_off) as *const PageTable);
                    let e3 = &l3[l3_idx];
                    if e3.is_unused() {
                        error!("P3 entry {} is unused", l3_idx);
                    } else {
                        error!(
                            "P3 entry {}: addr={:#x}, flags={:?}",
                            l3_idx,
                            e3.addr().as_u64(),
                            e3.flags()
                        );
                        let l2_phys = e3.addr().as_u64();
                        let l2 = &*((l2_phys + phys_off) as *const PageTable);
                        let e2 = &l2[l2_idx];
                        if e2.is_unused() {
                            error!("P2 entry {} is unused", l2_idx);
                        } else {
                            error!(
                                "P2 entry {}: addr={:#x}, flags={:?}",
                                l2_idx,
                                e2.addr().as_u64(),
                                e2.flags()
                            );
                            let l1_phys = e2.addr().as_u64();
                            let l1 = &*((l1_phys + phys_off) as *const PageTable);
                            let e1 = &l1[l1_idx];
                            if e1.is_unused() {
                                error!("P1 entry {} is unused", l1_idx);
                            } else {
                                error!(
                                    "P1 entry {}: addr={:#x}, flags={:?}",
                                    l1_idx,
                                    e1.addr().as_u64(),
                                    e1.flags()
                                );
                            }
                        }
                    }
                }

                // Use stack_frame for RSP (avoid heavy asm) and capture a couple of regs
                let rsp_val = stack_frame.stack_pointer.as_u64();

                let (rax, rbx) = {
                    let mut rax: u64 = 0;
                    let mut rbx: u64 = 0;
                    core::arch::asm!(
                        "mov {0}, rax",
                        "mov {1}, rbx",
                        out(reg) rax, out(reg) rbx,
                        options(nostack, preserves_flags),
                    );
                    (rax, rbx)
                };

                error!(
                    "Registers (partial): RAX={:#x} RBX={:#x} RSP={:#x}",
                    rax, rbx, rsp_val
                );

                let mut stack_words = [0u64; 8];
                for i in 0..stack_words.len() {
                    let va = VirtAddr::new(rsp_val + (i as u64) * 8);
                    if let Some(pa) = pt.translate_addr(va) {
                        let kaddr = (pa.as_u64() + phys_off) as *const u64;
                        stack_words[i] = core::ptr::read_volatile(kaddr);
                    } else {
                        stack_words[i] = 0xffffffffffffffffu64;
                    }
                }
                error!("Stack @ RSP {:#x}: {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x}",
                    rsp_val,
                    stack_words[0], stack_words[1], stack_words[2], stack_words[3],
                    stack_words[4], stack_words[5], stack_words[6], stack_words[7],
                );

                for (i, &w) in stack_words.iter().enumerate() {
                    if w >= 0x4000_0000 && w < 0x5000_0000 {
                        let func_va = VirtAddr::new(w + 0x40);
                        if let Some(pa2) = pt.translate_addr(func_va) {
                            let kptr = (pa2.as_u64() + phys_off) as *const u64;
                            let funcptr = core::ptr::read_volatile(kptr);
                            error!(
                                "Possible FILE at stack[{}] {:#x}: funcptr[+0x40] = {:#x}",
                                i, w, funcptr
                            );
                        } else {
                            error!(
                                "Possible FILE at stack[{}] {:#x}: funcptr[+0x40] not mapped",
                                i, w
                            );
                        }

                        // Dump first 16 bytes at the candidate heap address for inspection
                        let mut b = [0u8; 16];
                        for j in 0..b.len() {
                            let bva = VirtAddr::new(w + j as u64);
                            if let Some(bpa) = pt.translate_addr(bva) {
                                let bk = (bpa.as_u64() + phys_off) as *const u8;
                                b[j] = core::ptr::read_volatile(bk);
                            } else {
                                b[j] = 0xff;
                            }
                        }
                        error!("Bytes @ {:#x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                            w,
                            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
                        );
                    }
                }
            }
        } else {
            error!("Cannot get physical_memory_offset()");
        }

        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// デバイス利用不可例外ハンドラ
///
/// デバイス利用不可例外は、FPUやSIMD命令を使用しようとしたときに、対応するデバイスが利用できない場合に発生する。通常はFPUの初期化が必要な場合に発生することが多い
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn device_not_available_handler(stack_frame: InterruptStackFrame) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    error!(
        "EXCEPTION: DEVICE NOT AVAILABLE ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("{:#?}", stack_frame);
    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// ダブルフォルト例外ハンドラ
///
/// ダブルフォルトは、例外が発生した際にさらに例外が発生した場合に発生する。通常はスタックオーバーフローや重大なシステムエラーが原因で発生することが多い。
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
/// - `error_code`: ダブルフォルトのエラーコード（通常は0だが、特定の条件下で値が設定されることがある）
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    error!("EXCEPTION: DOUBLE FAULT");
    error!("Error code: {:#x}", error_code);
    error!("{:#?}", stack_frame);
    halt_forever();
}

/// TSS無効例外ハンドラ
///
/// TSS無効例外は、タスクスイッチングや特定のスタック操作が行われた際に、TSSが無効である場合に発生する。通常はTSSの設定ミスや不正なタスクスイッチングが原因で発生することが多い。
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
/// - `error_code`: TSS無効例外のエラーコード（通常は0だが、特定の条件下で値が設定されることがある）
extern "x86-interrupt" fn invalid_tss_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    error!(
        "EXCEPTION: INVALID TSS ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("Error code: {:#x}", error_code);
    error!("{:#?}", stack_frame);
    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// セグメント不存在例外ハンドラ
///
/// セグメント不存在例外は、セグメントレジスタが無効なセグメントを指している場合に発生する。通常はGDTやLDTの設定ミスが原因で発生することが多い。
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
/// - `error_code`: セグメント不存在例外のエラーコード（通常は0だが、特定の条件下で値が設定されることがある）
extern "x86-interrupt" fn segment_not_present_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    error!(
        "EXCEPTION: SEGMENT NOT PRESENT ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("Error code: {:#x}", error_code);
    error!("{:#?}", stack_frame);
    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// スタックセグメントフォルト例外ハンドラ
///
/// スタックセグメントフォルトは、スタックセグメントにアクセスしようとした際に、スタックセグメントが無効である場合に発生する。通常はスタックオーバーフローや不正なスタック操作が原因で発生することが多い。
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
/// - `error_code`: スタックセグメントフォルトのエラーコード（通常は0だが、特定の条件下で値が設定されることがある）
extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;

    error!(
        "EXCEPTION: STACK SEGMENT FAULT ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("Error code: {:#x}", error_code);
    warn!("{:#?}", stack_frame);

    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// 一般保護例外ハンドラ
///
/// 一般保護例外は、セグメント違反やアクセス違反などの保護違反が発生した場合に発生する。ユーザーモードで発生した場合はプロセスを終了させ、カーネルモードで発生した場合はシステム全体を停止する。
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
/// - `error_code`: 一般保護例外のエラーコード（エラーコードのビットフィールドには、外部割り込みか、どのテーブルからの例外かなどの情報が含まれる）
extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;

    error!(
        "EXCEPTION: GENERAL PROTECTION FAULT ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("Error code: {:#x}", error_code);

    // エラーコードの詳細を解析
    let external = (error_code & 0x1) != 0;
    let table = (error_code >> 1) & 0x3;
    let index = (error_code >> 3) & 0x1FFF;

    error!(
        "  External: {}, Table: {} ({}), Index: {}",
        external,
        table,
        match table {
            0 => "GDT",
            1 => "IDT",
            2 | 3 => "LDT",
            _ => "Unknown",
        },
        index
    );

    warn!("{:#?}", stack_frame);

    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// ページフォルト例外ハンドラ
///
/// ページフォルトは、仮想メモリ管理に関連する例外で、アクセス違反やページの不在などが原因で発生する。ユーザーモードで発生した場合はプロセスを終了させ、カーネルモードで発生した場合はシステム全体を停止する。
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
/// - `error_code`: ページフォルトのエラーコード（エラーコードのビットフィールドには、ページが存在しないか、書き込みアクセスか、ユーザーモードかなどの情報が含まれる）
extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: x86_64::structures::idt::PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    use x86_64::VirtAddr;

    let faulting_addr = Cr2::read().unwrap_or(VirtAddr::new(0));
    let is_user_mode = error_code.contains(x86_64::structures::idt::PageFaultErrorCode::USER_MODE);

    error!(
        "EXCEPTION: PAGE FAULT ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("Accessed address: {:#x}", faulting_addr.as_u64());
    error!("Error code: {:?}", error_code);
    error!(
        "  Present: {}, Write: {}, User: {}, Reserved: {}, Instruction: {}",
        error_code.contains(x86_64::structures::idt::PageFaultErrorCode::PROTECTION_VIOLATION),
        error_code.contains(x86_64::structures::idt::PageFaultErrorCode::CAUSED_BY_WRITE),
        is_user_mode,
        error_code.contains(x86_64::structures::idt::PageFaultErrorCode::MALFORMED_TABLE),
        error_code.contains(x86_64::structures::idt::PageFaultErrorCode::INSTRUCTION_FETCH)
    );

    if let Some(phys) = crate::mem::paging::translate_addr(faulting_addr) {
        error!(
            "  Virtual {:#x} is mapped to physical {:#x}",
            faulting_addr.as_u64(),
            phys.as_u64()
        );
    } else {
        error!("  Virtual {:#x} is NOT mapped", faulting_addr.as_u64());
    }

    if is_user_mode {
        // ユーザーモードでのページフォルト: プロセスを終了
        error!("Terminating faulting user process");
        debug!("{:#?}", stack_frame);

        // 現在のプロセスを終了させる
        crate::task::scheduler::exit_current_process(-1);
    } else {
        // カーネルモードでのページフォルト: システム全体を停止
        error!("FATAL: Page fault in kernel mode!");
        error!("{:#?}", stack_frame);
        error!("Please report this to https://github.com/tas0dev/SwiftCore/issues with the above log details. :(");
        halt_cpu();
    }
}

/// x87浮動小数点例外ハンドラ
///
/// x87浮動小数点例外は、x87 FPU命令の実行中にエラーが発生した場合に発生する。通常はFPUの状態が不正な場合や、無効な操作が行われた場合に発生することが多い。
/// ユーザーモードで発生した場合はプロセスを終了させ、カーネルモードで発生した場合はシステム全体を停止する。
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn x87_floating_point_handler(stack_frame: InterruptStackFrame) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    error!(
        "EXCEPTION: X87 FLOATING POINT ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("{:#?}", stack_frame);
    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// アライメントチェック例外ハンドラ
///
/// アライメントチェック例外は、特定のデータアクセスが適切にアライメントされていない場合に発生する。通常は、CPUが要求するアライメント要件を満たさないメモリアクセスが原因で発生することが多い。
/// ユーザーモードで発生した場合はプロセスを終了させ、カーネルモードで発生した場合はシステム全体を停止する。
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
/// - `error_code`: アライメントチェック例外のエラーコード（エラーコードのビットフィールドには、ユーザーモードか、外部割り込みかなどの情報が含まれる）
extern "x86-interrupt" fn alignment_check_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    error!(
        "EXCEPTION: ALIGNMENT CHECK ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("Error code: {:#x}", error_code);
    error!("{:#?}", stack_frame);
    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// マシンチェック例外ハンドラ
///
/// マシンチェック例外は、ハードウェアの障害や重大なエラーが発生した場合に発生する。通常はCPUやメモリの障害、電源の問題などが原因で発生することが多い。
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn machine_check_handler(stack_frame: InterruptStackFrame) -> ! {
    error!("EXCEPTION: MACHINE CHECK");
    error!("{:#?}", stack_frame);
    halt_forever();
}

/// SIMD浮動小数点例外ハンドラ
/// SIMD浮動小数点例外は、SIMD命令の実行中にエラーが発生した場合に発生する。通常は、SIMDレジスタの状態が不正な場合や、無効な操作が行われた場合に発生することが多い。
/// ユーザーモードで発生した場合はプロセスを終了させ、カーネルモードで発生した場合はシステム全体を停止する。
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn simd_floating_point_handler(stack_frame: InterruptStackFrame) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    error!(
        "EXCEPTION: SIMD FLOATING POINT ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("{:#?}", stack_frame);
    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// 仮想化例外ハンドラ
/// 仮想化例外は、仮想化機能を使用している環境で、仮想化関連のエラーが発生した場合に発生する。通常は、仮想化機能の設定ミスや、仮想化環境でサポートされていない操作が原因で発生することが多い。
/// ユーザーモードで発生した場合はプロセスを終了させ、カーネルモードで発生した場合はシステム全体を停止する。
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn virtualization_handler(stack_frame: InterruptStackFrame) {
    let is_user_mode = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    error!(
        "EXCEPTION: VIRTUALIZATION ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    error!("{:#?}", stack_frame);
    if is_user_mode {
        error!("Terminating faulting user process");
        crate::task::scheduler::exit_current_process(-1);
    } else {
        halt_cpu();
    }
}

/// キーボード割り込みハンドラ (IRQ1 / ベクタ 33)
///
/// IRQ1 をIDTに登録せずに放置するとキーストロークのたびに #GP が発生し
/// OS全体が停止する (C-2修正)。このハンドラはスキャンコードを読み捨て EOI を送る。
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // スキャンコードを読み取りコントローラをクリアする
    let _scancode: u8 = unsafe {
        let mut port = x86_64::instructions::port::Port::<u8>::new(0x60);
        port.read()
    };
    // マスターPICにEOIを送信 (IRQ1はマスターPICが担当)
    unsafe {
        super::pic::PIC_MASTER.end_of_interrupt();
    }
}

/// 一般的な割り込みハンドラ（スタブ）
///
/// 一般的なハードウェア割り込み（例: キーボード、マウス、ネットワークカードなど）を処理するためのスタブハンドラ
/// とりあえず、割り込みが発生したことをログに出力し、EOIを送信するだけの簡単な実装
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
///
/// このハンドラは、将来的に各デバイスに対応した具体的な処理を実装するためのプレースホルダとして使用される予定
extern "x86-interrupt" fn generic_interrupt_handler(_stack_frame: InterruptStackFrame) {
    debug!("INTERRUPT: GENERIC");
    // マスターPICのみにEOIを送信する (LOW-01)
    // このハンドラはどのIRQから呼ばれるか不明のため、IRQ 0-7 (マスターのみ) を想定して
    // スレーブPICへの不正なEOI送信によるスプリアス割り込みを防ぐ。
    // IRQ 8-15 が必要なデバイスは専用ハンドラで両PICにEOIを送る。
    unsafe {
        super::pic::PIC_MASTER.end_of_interrupt();
    }
}

/// CPU割り込みを無効化してシステムを停止
fn halt_cpu() {
    x86_64::instructions::interrupts::disable();
    loop {
        x86_64::instructions::hlt();
    }
}

/// CPU割り込みを無効化してシステムを停止（戻らない）
fn halt_forever() -> ! {
    x86_64::instructions::interrupts::disable();
    loop {
        x86_64::instructions::hlt();
    }
}
