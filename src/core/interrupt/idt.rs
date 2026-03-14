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
        idt[33].set_handler_fn(keyboard_interrupt_handler); // Keyboard IRQ1 (C-2修正)

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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
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

fn read_phys_u8(phys_off: u64, phys_addr: u64) -> Option<u8> {
    let virt = phys_addr.checked_add(phys_off)? as *const u8;
    Some(unsafe { core::ptr::read_volatile(virt) })
}

fn read_phys_u64_le(phys_off: u64, phys_addr: u64) -> Option<u64> {
    let mut bytes = [0u8; 8];
    for (i, b) in bytes.iter_mut().enumerate() {
        let addr = phys_addr.checked_add(i as u64)?;
        *b = read_phys_u8(phys_off, addr)?;
    }
    Some(u64::from_le_bytes(bytes))
}

fn read_page_table_entry(phys_off: u64, table_phys: u64, index: usize) -> Option<u64> {
    let entry_addr = table_phys.checked_add((index as u64).checked_mul(8)?)?;
    read_phys_u64_le(phys_off, entry_addr)
}

fn log_page_table_entry(level: &str, index: usize, entry: u64) {
    const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
    error!(
        "{} entry {}: raw={:#018x}, addr={:#x}, P={}, W={}, U={}, PS={}, NX={}",
        level,
        index,
        entry,
        entry & ADDR_MASK,
        (entry & (1 << 0)) != 0,
        (entry & (1 << 1)) != 0,
        (entry & (1 << 2)) != 0,
        (entry & (1 << 7)) != 0,
        (entry & (1 << 63)) != 0
    );
}

fn translate_virt_to_phys(l4_phys: u64, phys_off: u64, virt_addr: u64) -> Option<u64> {
    use x86_64::VirtAddr;

    const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

    let _ = VirtAddr::try_new(virt_addr).ok()?;

    let l4_idx = ((virt_addr >> 39) & 0x1ff) as usize;
    let l3_idx = ((virt_addr >> 30) & 0x1ff) as usize;
    let l2_idx = ((virt_addr >> 21) & 0x1ff) as usize;
    let l1_idx = ((virt_addr >> 12) & 0x1ff) as usize;

    let e4 = read_page_table_entry(phys_off, l4_phys, l4_idx)?;
    if (e4 & 1) == 0 {
        return None;
    }

    let l3_phys = e4 & ADDR_MASK;
    let e3 = read_page_table_entry(phys_off, l3_phys, l3_idx)?;
    if (e3 & 1) == 0 {
        return None;
    }
    if (e3 & (1 << 7)) != 0 {
        let base = e3 & 0x000f_ffff_c000_0000;
        return Some(base | (virt_addr & 0x3fff_ffff));
    }

    let l2_phys = e3 & ADDR_MASK;
    let e2 = read_page_table_entry(phys_off, l2_phys, l2_idx)?;
    if (e2 & 1) == 0 {
        return None;
    }
    if (e2 & (1 << 7)) != 0 {
        let base = e2 & 0x000f_ffff_ffe0_0000;
        return Some(base | (virt_addr & 0x1f_ffff));
    }

    let l1_phys = e2 & ADDR_MASK;
    let e1 = read_page_table_entry(phys_off, l1_phys, l1_idx)?;
    if (e1 & 1) == 0 {
        return None;
    }

    let base = e1 & ADDR_MASK;
    Some(base | (virt_addr & 0xfff))
}

fn read_virtual_u8(l4_phys: u64, phys_off: u64, virt_addr: u64) -> Option<u8> {
    let phys = translate_virt_to_phys(l4_phys, phys_off, virt_addr)?;
    read_phys_u8(phys_off, phys)
}

fn read_virtual_u64_le(l4_phys: u64, phys_off: u64, virt_addr: u64) -> Option<u64> {
    let mut bytes = [0u8; 8];
    for (i, b) in bytes.iter_mut().enumerate() {
        let addr = virt_addr.checked_add(i as u64)?;
        *b = read_virtual_u8(l4_phys, phys_off, addr)?;
    }
    Some(u64::from_le_bytes(bytes))
}

fn dump_invalid_opcode_diagnostics(stack_frame: &InterruptStackFrame) {
    use x86_64::registers::control::Cr3;

    const DUMP_BYTES: usize = 16;
    const STACK_WORDS: usize = 8;
    const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
    const UNREADABLE_BYTE: u8 = 0xff;
    const UNREADABLE_WORD: u64 = u64::MAX;

    error!("===== INVALID OPCODE DEBUG DUMP BEGIN =====");
    error!("{:#?}", stack_frame);

    let current_tid = crate::task::current_thread_id();
    error!("Current context: tid={:?}", current_tid);

    let Some(phys_off) = crate::mem::paging::physical_memory_offset() else {
        error!("Cannot dump address-space details: physical_memory_offset unavailable");
        error!("===== INVALID OPCODE DEBUG DUMP END =====");
        return;
    };

    let (cr3_frame, _) = Cr3::read();
    let l4_phys = cr3_frame.start_address().as_u64();
    error!("Active CR3 L4 physical address: {:#x}", l4_phys);

    let rip = stack_frame.instruction_pointer.as_u64();
    let rsp = stack_frame.stack_pointer.as_u64();

    if let Some(pa) = translate_virt_to_phys(l4_phys, phys_off, rip) {
        error!("RIP mapping: virt={:#x} -> phys={:#x}", rip, pa);
    } else {
        error!(
            "RIP mapping: virt={:#x} is not mapped or non-canonical",
            rip
        );
    }

    let mut inst = [UNREADABLE_BYTE; DUMP_BYTES];
    for (i, b) in inst.iter_mut().enumerate() {
        let Some(addr) = rip.checked_add(i as u64) else {
            break;
        };
        if let Some(v) = read_virtual_u8(l4_phys, phys_off, addr) {
            *b = v;
        }
    }
    error!(
        "Instruction bytes @ {:#x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
        rip,
        inst[0], inst[1], inst[2], inst[3],
        inst[4], inst[5], inst[6], inst[7],
        inst[8], inst[9], inst[10], inst[11],
        inst[12], inst[13], inst[14], inst[15],
    );

    let l4_idx = ((rip >> 39) & 0x1ff) as usize;
    let l3_idx = ((rip >> 30) & 0x1ff) as usize;
    let l2_idx = ((rip >> 21) & 0x1ff) as usize;
    let l1_idx = ((rip >> 12) & 0x1ff) as usize;

    if let Some(e4) = read_page_table_entry(phys_off, l4_phys, l4_idx) {
        log_page_table_entry("P4", l4_idx, e4);
        if (e4 & 1) != 0 {
            let l3_phys = e4 & ADDR_MASK;
            if let Some(e3) = read_page_table_entry(phys_off, l3_phys, l3_idx) {
                log_page_table_entry("P3", l3_idx, e3);
                if (e3 & 1) != 0 {
                    if (e3 & (1 << 7)) != 0 {
                        error!("P3 entry indicates 1GiB huge page mapping");
                    } else {
                        let l2_phys = e3 & ADDR_MASK;
                        if let Some(e2) = read_page_table_entry(phys_off, l2_phys, l2_idx) {
                            log_page_table_entry("P2", l2_idx, e2);
                            if (e2 & 1) != 0 {
                                if (e2 & (1 << 7)) != 0 {
                                    error!("P2 entry indicates 2MiB huge page mapping");
                                } else {
                                    let l1_phys = e2 & ADDR_MASK;
                                    if let Some(e1) =
                                        read_page_table_entry(phys_off, l1_phys, l1_idx)
                                    {
                                        log_page_table_entry("P1", l1_idx, e1);
                                    } else {
                                        error!("Failed to read P1 entry {}", l1_idx);
                                    }
                                }
                            } else {
                                error!("P2 entry {} is not present", l2_idx);
                            }
                        } else {
                            error!("Failed to read P2 entry {}", l2_idx);
                        }
                    }
                } else {
                    error!("P3 entry {} is not present", l3_idx);
                }
            } else {
                error!("Failed to read P3 entry {}", l3_idx);
            }
        } else {
            error!("P4 entry {} is not present", l4_idx);
        }
    } else {
        error!("Failed to read P4 entry {}", l4_idx);
    }

    let mut stack_words = [UNREADABLE_WORD; STACK_WORDS];
    for (i, w) in stack_words.iter_mut().enumerate() {
        let Some(addr) = rsp.checked_add((i as u64) * 8) else {
            break;
        };
        if let Some(v) = read_virtual_u64_le(l4_phys, phys_off, addr) {
            *w = v;
        }
    }
    error!(
        "Stack @ RSP {:#x}: {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x}",
        rsp,
        stack_words[0], stack_words[1], stack_words[2], stack_words[3],
        stack_words[4], stack_words[5], stack_words[6], stack_words[7],
    );

    for (i, &candidate) in stack_words.iter().enumerate() {
        if (0x4000_0000u64..0x5000_0000u64).contains(&candidate) {
            let func_va = candidate.checked_add(0x40);
            match func_va.and_then(|addr| read_virtual_u64_le(l4_phys, phys_off, addr)) {
                Some(func_ptr) => {
                    error!(
                        "Possible FILE at stack[{}] {:#x}: funcptr[+0x40] = {:#x}",
                        i, candidate, func_ptr
                    );
                }
                None => {
                    error!(
                        "Possible FILE at stack[{}] {:#x}: funcptr[+0x40] unreadable",
                        i, candidate
                    );
                }
            }

            let mut bytes = [UNREADABLE_BYTE; DUMP_BYTES];
            for (j, b) in bytes.iter_mut().enumerate() {
                if let Some(addr) = candidate.checked_add(j as u64) {
                    if let Some(v) = read_virtual_u8(l4_phys, phys_off, addr) {
                        *b = v;
                    }
                }
            }
            error!(
                "Bytes @ {:#x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                candidate,
                bytes[0], bytes[1], bytes[2], bytes[3],
                bytes[4], bytes[5], bytes[6], bytes[7],
                bytes[8], bytes[9], bytes[10], bytes[11],
                bytes[12], bytes[13], bytes[14], bytes[15],
            );
        }
    }

    error!("===== INVALID OPCODE DEBUG DUMP END =====");
}

/// 無効命令例外ハンドラ
///
/// 無効命令例外は、CPUが認識できない命令が実行されたときに発生する。ユーザーモードで発生した場合はプロセスを終了させ、カーネルモードで発生した場合はシステム全体を停止する
///
/// ## Arguments
/// - `stack_frame`: 割り込み発生時のCPU状態を表す構造体
extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    // ユーザーモードかチェック（code_segmentのRPLビットを確認）
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;

    error!(
        "EXCEPTION: INVALID OPCODE ({})",
        if is_user_mode {
            "USER MODE"
        } else {
            "KERNEL MODE"
        }
    );
    dump_invalid_opcode_diagnostics(&stack_frame);

    if is_user_mode {
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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;

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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;

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
        if let Some(tid) = crate::task::current_thread_id() {
            if let Some((pid, name)) = crate::task::with_thread(tid, |t| {
                let pid = t.process_id();
                let name = crate::task::with_process(pid, |p| {
                    let mut s = alloc::string::String::new();
                    s.push_str(p.name());
                    s
                })
                .unwrap_or_else(|| alloc::string::String::from("<unknown>"));
                (pid, name)
            }) {
                error!(
                    "Faulting user context: pid={:?}, tid={:?}, process='{}', rip={:#x}, rsp={:#x}",
                    pid,
                    tid,
                    name,
                    stack_frame.instruction_pointer.as_u64(),
                    stack_frame.stack_pointer.as_u64()
                );
            }
        }

        // 保護違反（既マップページへの不正アクセス）でなければスタック拡張を試みる
        let is_protection_violation = error_code
            .contains(x86_64::structures::idt::PageFaultErrorCode::PROTECTION_VIOLATION);
        if !is_protection_violation {
            if try_grow_user_stack(faulting_addr.as_u64()) {
                return; // スタック拡張成功 → 命令を再試行
            }
        }

        error!("Terminating faulting user process");
        debug!("{:#?}", stack_frame);
        crate::task::scheduler::exit_current_process(-1);
    } else {
        // カーネルモードでのページフォルト: システム全体を停止
        error!("FATAL: Page fault in kernel mode!");
        error!("{:#?}", stack_frame);
        error!("Please report this to https://github.com/tas0dev/mochiOS/issues with the above log details. :(");
        halt_cpu();
    }
}

/// ユーザースタックの自動拡張を試みる。
/// fault_addr がスタック下端の直下にある場合、新しいページをマップして true を返す。
/// 最大スタックサイズは 8 MiB。
fn try_grow_user_stack(fault_addr: u64) -> bool {
    const MAX_STACK_SIZE: u64 = 8 * 1024 * 1024; // 8 MiB

    let tid = match crate::task::current_thread_id() {
        Some(t) => t,
        None => return false,
    };
    let pid = match crate::task::with_thread(tid, |t| t.process_id()) {
        Some(p) => p,
        None => return false,
    };
    let (stack_bottom, stack_top, page_table) =
        match crate::task::with_process(pid, |p| (p.stack_bottom(), p.stack_top(), p.page_table()))
        {
            Some(v) => v,
            None => return false,
        };
    let page_table = match page_table {
        Some(pt) => pt,
        None => return false,
    };
    if stack_bottom == 0 || stack_top == 0 {
        return false;
    }
    // フォルトアドレスは現在のスタック下端より下でなければならない
    if fault_addr >= stack_bottom {
        return false;
    }
    // 最大スタックサイズを超えて伸ばさない
    let min_allowed = stack_top.saturating_sub(MAX_STACK_SIZE);
    if fault_addr < min_allowed {
        crate::error!(
            "Stack overflow: fault at {:#x}, min allowed {:#x}",
            fault_addr,
            min_allowed
        );
        return false;
    }
    // フォルトページから現在の下端まで一括マップ（通常は1ページだけ）
    let new_page = (fault_addr / 4096) * 4096;
    let map_size = stack_bottom - new_page;
    if crate::mem::paging::map_and_copy_segment_to(page_table, new_page, 0, map_size, &[], true, false)
        .is_ok()
    {
        crate::task::with_process_mut(pid, |p| p.set_stack_bottom(new_page));
        crate::debug!("Stack grown: {:#x} -> {:#x}", stack_bottom, new_page);
        true
    } else {
        false
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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
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
    let is_user_mode = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
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
    let scancode: u8 = unsafe {
        let mut port = x86_64::instructions::port::Port::<u8>::new(0x60);
        port.read()
    };
    crate::util::ps2kbd::push_scancode(scancode);
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
