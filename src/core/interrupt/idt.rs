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
        idt[32].set_handler_fn(super::timer::timer_interrupt_handler); // Timer
        idt[33].set_handler_fn(keyboard_interrupt_handler); // Keyboard

        // それ以外のハードウェア割り込みはとりあえずスタブ
        for i in 34..48 {
            idt[i].set_handler_fn(generic_interrupt_handler);
        }

        // システムコール割り込み (0x80)
        // naked functionなので、手動で設定
        unsafe {
            let handler_addr = syscall::syscall_interrupt_handler as *const () as u64;
            idt[0x80].set_handler_addr(x86_64::VirtAddr::new(handler_addr))
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

// ========================================
// CPU例外ハンドラ
// ========================================

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    error!("EXCEPTION: DIVIDE ERROR");
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn debug_handler(stack_frame: InterruptStackFrame) {
    debug!("EXCEPTION: DEBUG");
    debug!("{:#?}", stack_frame);
}

extern "x86-interrupt" fn nmi_handler(stack_frame: InterruptStackFrame) {
    error!("EXCEPTION: NON-MASKABLE INTERRUPT");
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    warn!("EXCEPTION: BREAKPOINT");
    debug!("{:#?}", stack_frame);
}

extern "x86-interrupt" fn overflow_handler(stack_frame: InterruptStackFrame) {
    error!("EXCEPTION: OVERFLOW");
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn bound_range_exceeded_handler(stack_frame: InterruptStackFrame) {
    error!("EXCEPTION: BOUND RANGE EXCEEDED");
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    error!("EXCEPTION: INVALID OPCODE");
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn device_not_available_handler(stack_frame: InterruptStackFrame) {
    error!("EXCEPTION: DEVICE NOT AVAILABLE");
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    error!("EXCEPTION: DOUBLE FAULT");
    error!("Error code: {:#x}", error_code);
    debug!("{:#?}", stack_frame);
    halt_forever();
}

extern "x86-interrupt" fn invalid_tss_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    error!("EXCEPTION: INVALID TSS");
    error!("Error code: {:#x}", error_code);
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn segment_not_present_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    error!("EXCEPTION: SEGMENT NOT PRESENT");
    error!("Error code: {:#x}", error_code);
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    error!("EXCEPTION: STACK SEGMENT FAULT");
    error!("Error code: {:#x}", error_code);
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    error!("EXCEPTION: GENERAL PROTECTION FAULT");
    error!("Error code: {:#x}", error_code);

    // エラーコードの詳細を解析
    let external = (error_code & 0x1) != 0;
    let table = (error_code >> 1) & 0x3;
    let index = (error_code >> 3) & 0x1FFF;

    error!("  External: {}, Table: {} ({}), Index: {}",
           external,
           table,
           match table {
               0 => "GDT",
               1 => "IDT",
               2 | 3 => "LDT",
               _ => "Unknown",
           },
           index);

    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: x86_64::structures::idt::PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    use x86_64::VirtAddr;

    let faulting_addr = Cr2::read().unwrap_or(VirtAddr::new(0));

    error!("EXCEPTION: PAGE FAULT");
    error!("Accessed address: {:#x}", faulting_addr.as_u64());
    error!("Error code: {:?}", error_code);
    error!("  Present: {}, Write: {}, User: {}, Reserved: {}, Instruction: {}",
           error_code.contains(x86_64::structures::idt::PageFaultErrorCode::PROTECTION_VIOLATION),
           error_code.contains(x86_64::structures::idt::PageFaultErrorCode::CAUSED_BY_WRITE),
           error_code.contains(x86_64::structures::idt::PageFaultErrorCode::USER_MODE),
           error_code.contains(x86_64::structures::idt::PageFaultErrorCode::MALFORMED_TABLE),
           error_code.contains(x86_64::structures::idt::PageFaultErrorCode::INSTRUCTION_FETCH));

    // フォルトしたアドレスの周辺のページテーブルエントリを確認
    if let Some(phys) = crate::mem::paging::translate_addr(faulting_addr) {
        error!("  Virtual {:#x} is mapped to physical {:#x}", faulting_addr.as_u64(), phys.as_u64());
    } else {
        error!("  Virtual {:#x} is NOT mapped", faulting_addr.as_u64());
    }

    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn x87_floating_point_handler(stack_frame: InterruptStackFrame) {
    error!("EXCEPTION: X87 FLOATING POINT");
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn alignment_check_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    error!("EXCEPTION: ALIGNMENT CHECK");
    error!("Error code: {:#x}", error_code);
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn machine_check_handler(stack_frame: InterruptStackFrame) -> ! {
    error!("EXCEPTION: MACHINE CHECK");
    debug!("{:#?}", stack_frame);
    halt_forever();
}

extern "x86-interrupt" fn simd_floating_point_handler(stack_frame: InterruptStackFrame) {
    error!("EXCEPTION: SIMD FLOATING POINT");
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

extern "x86-interrupt" fn virtualization_handler(stack_frame: InterruptStackFrame) {
    error!("EXCEPTION: VIRTUALIZATION");
    debug!("{:#?}", stack_frame);
    halt_cpu();
}

// ========================================
// ハードウェア割り込みハンドラ
// ========================================

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    debug!("INTERRUPT: KEYBOARD");
    // キーボード入力を処理
    // TODO: キーボードドライバ実装
    super::send_eoi(33);
}

extern "x86-interrupt" fn generic_interrupt_handler(_stack_frame: InterruptStackFrame) {
    debug!("INTERRUPT: GENERIC");
    // EOIを送信
    unsafe {
        super::pic::PIC_SLAVE.end_of_interrupt();
        super::pic::PIC_MASTER.end_of_interrupt();
    }
}

// ========================================
// ヘルパー関数
// ========================================

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
