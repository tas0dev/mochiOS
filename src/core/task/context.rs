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
        "mov [rdi + 0x00], rax",
        "mov [rdi + 0x08], rbp",
        "mov [rdi + 0x10], rbx",
        "mov [rdi + 0x18], r12",
        "mov [rdi + 0x20], r13",
        "mov [rdi + 0x28], r14",
        "mov [rdi + 0x30], r15",
        "mov rax, [rsp]",
        "mov [rdi + 0x38], rax",
        "pushfq",
        "pop rax",
        "mov [rdi + 0x40], rax",
        // restore new
        "mov rax, [rsi + 0x38]",
        "mov r11, [rsi + 0x40]",
        "mov rbx, [rsi + 0x10]",
        "mov r12, [rsi + 0x18]",
        "mov r13, [rsi + 0x20]",
        "mov r14, [rsi + 0x28]",
        "mov r15, [rsi + 0x30]",
        "mov rbp, [rsi + 0x08]",
        "mov rsp, [rsi + 0x00]",
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

    crate::info!(
        "switch_to_thread pointers: old={:?}, new={:?}",
        old_ctx_ptr,
        new_ctx_ptr
    );

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
        unsafe {
            // dump contexts for debug
            if !old_ctx_ptr.is_null() {
                let old = &*old_ctx_ptr;
                let new = &*new_ctx_ptr;
                crate::info!(
                    "old_ctx: rsp={:#x}, rbp={:#x}, rbx={:#x}, r12={:#x}, r13={:#x}, r14={:#x}, r15={:#x}, rip={:#x}, rflags={:#x}",
                    old.rsp, old.rbp, old.rbx, old.r12, old.r13, old.r14, old.r15, old.rip, old.rflags
                );
                crate::info!(
                    "new_ctx: rsp={:#x}, rbp={:#x}, rbx={:#x}, r12={:#x}, r13={:#x}, r14={:#x}, r15={:#x}, rip={:#x}, rflags={:#x}",
                    new.rsp, new.rbp, new.rbx, new.r12, new.r13, new.r14, new.r15, new.rip, new.rflags
                );
            }
        }

        switch_context(old_ctx_ptr, new_ctx_ptr);
    }
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

    crate::info!(
        "switch_from_isr: next_priv={:?}, saved: rsp={:#x}, rbp={:#x}, rip={:#x}, rflags={:#x}",
        next_priv,
        saved.rsp,
        saved.rbp,
        saved.rip,
        saved.rflags
    );

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

        crate::info!(
            "iretq prepare: user_cs={:#x}, user_ds={:#x}, user_rsp={:#x}, user_rip={:#x}, user_rflags={:#x}",
            user_cs,
            user_ds,
            ctx.rsp,
            ctx.rip,
            ctx.rflags
        );

        // Dump GDTR and first few GDT entries for diagnosis
        unsafe {
            let mut gdtr: [u8; 10] = [0; 10];
            core::arch::asm!("sgdt [{}]", in(reg) &mut gdtr, options(nostack));
            let limit = u16::from_le_bytes([gdtr[0], gdtr[1]]) as usize;
            let base = u64::from_le_bytes([
                gdtr[2], gdtr[3], gdtr[4], gdtr[5], gdtr[6], gdtr[7], gdtr[8], gdtr[9],
            ]);
            crate::info!("GDTR: base={:#x}, limit={:#x}", base, limit);
            // Dump first 8 descriptors (or as many as limit allows)
            let entries = core::cmp::min(8, (limit + 1) / 8);
            for i in 0..entries {
                let addr = (base + (i * 8) as u64) as *const u8;
                let desc_bytes = core::slice::from_raw_parts(addr, 8);
                let mut v: u64 = 0;
                for (j, b) in desc_bytes.iter().enumerate() {
                    v |= (*b as u64) << (j * 8);
                }
                // decode basic fields
                let limit_low = (v & 0xffff) as u64;
                let base_low = ((v >> 16) & 0xffffff) as u64;
                let access = ((v >> 40) & 0xff) as u8;
                let flags_limit_high = ((v >> 48) & 0xff) as u8;
                let base_high = ((v >> 56) & 0xff) as u8;
                let limit = limit_low | (((flags_limit_high & 0x0f) as u64) << 16);
                let base = base_low | ((base_high as u64) << 24);
                let flags = flags_limit_high >> 4;

                crate::info!(
                    "GDT[{}] = {:#018x} base={:#x} limit={:#x} access={:#04x} flags={:#x}",
                    i,
                    v,
                    base,
                    limit,
                    access,
                    flags
                );
            }
            // Check page table translation for user RIP and RSP
            use x86_64::VirtAddr;
            if let Some(code_phys) = crate::mem::paging::translate_addr(VirtAddr::new(ctx.rip)) {
                crate::info!("user RIP {:#x} -> phys {:#x}", ctx.rip, code_phys.as_u64());
            } else {
                crate::info!("user RIP {:#x} not mapped", ctx.rip);
            }
            if let Some(stack_phys) = crate::mem::paging::translate_addr(VirtAddr::new(ctx.rsp)) {
                crate::info!("user RSP {:#x} -> phys {:#x}", ctx.rsp, stack_phys.as_u64());
            } else {
                crate::info!("user RSP {:#x} not mapped", ctx.rsp);
            }

            // Decode and validate the specific selectors used for user mode entry
            let cs_sel = user_cs as u16;
            let ds_sel = user_ds as u16;
            let cs_index = (cs_sel >> 3) as usize;
            let ds_index = (ds_sel >> 3) as usize;
            crate::info!("user selectors: cs={:#x} (idx={}), ds={:#x} (idx={})", cs_sel, cs_index, ds_sel, ds_index);

            // Read descriptor bytes for those indices if available
            let mut dump_descriptor = |idx: usize| {
                let gdtr_base = base as usize;
                let desc_addr = gdtr_base + idx * 8;
                let desc_ptr = desc_addr as *const u8;
                let desc = core::slice::from_raw_parts(desc_ptr, 8);
                let mut v: u64 = 0;
                for (j, b) in desc.iter().enumerate() {
                    v |= (*b as u64) << (j * 8);
                }
                let limit_low = (v & 0xffff) as u64;
                let base_low = ((v >> 16) & 0xffffff) as u64;
                let access = ((v >> 40) & 0xff) as u8;
                let flags_limit_high = ((v >> 48) & 0xff) as u8;
                let base_high = ((v >> 56) & 0xff) as u8;
                let limit = limit_low | (((flags_limit_high & 0x0f) as u64) << 16);
                let base_field = base_low | ((base_high as u64) << 24);
                let flags_nibble = flags_limit_high >> 4;
                let present = (access & 0x80) != 0;
                let dpl = (access >> 5) & 0x3;
                let s_bit = (access >> 4) & 0x1;
                let executable = (access >> 3) & 0x1;
                let l_bit = (flags_nibble >> 1) & 0x1;
                crate::info!(
                    "GDT[idx={}]: raw={:#018x} base={:#x} limit={:#x} access={:#04x} flags={:#x} P={} DPL={} S={} X={} L={}",
                    idx, v, base_field, limit, access, flags_nibble, present, dpl, s_bit, executable, l_bit
                );
            };

            if (cs_index as usize) * 8 + 7 <= (limit as usize) {
                dump_descriptor(cs_index);
            } else {
                crate::info!("CS index {} out of GDTR limit", cs_index);
            }
            if (ds_index as usize) * 8 + 7 <= (limit as usize) {
                dump_descriptor(ds_index);
            } else {
                crate::info!("DS index {} out of GDTR limit", ds_index);
            }

            // Canonicality checks
            let check_canonical = |addr: u64| {
                let hi = addr >> 47;
                hi == 0 || hi == 0x1ffff
            };
            crate::info!("user RIP canonical={}" , check_canonical(ctx.rip));
            crate::info!("user RSP canonical={}" , check_canonical(ctx.rsp));

            // If mapped, dump a few bytes from code and stack (protect against unmapped)
            if crate::mem::paging::translate_addr(VirtAddr::new(ctx.rip)).is_some() {
                unsafe {
                    let p = ctx.rip as *const u8;
                    let mut bytes = [0u8; 16];
                    for i in 0..16 {
                        bytes[i] = core::ptr::read_volatile(p.add(i));
                    }
                    crate::info!("user code @ {:#x}: {:02x?}", ctx.rip, bytes);
                }
            }
            if crate::mem::paging::translate_addr(VirtAddr::new(ctx.rsp)).is_some() {
                unsafe {
                    let p = ctx.rsp as *const u8;
                    let mut bytes = [0u8; 32];
                    for i in 0..32 {
                        bytes[i] = core::ptr::read_volatile(p.add(i));
                    }
                    crate::info!("user stack @ {:#x}: {:02x?}", ctx.rsp, bytes);
                }
            }
        }

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
}
