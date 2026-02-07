use crate::elf::loader as elf_loader;

/// カーネル内から実行可能ファイルを読み込み実行するシステムコール
pub fn exec_kernel(path_ptr: u64) -> u64 {
    let mut provided_path: Option<&str> = None;
    if path_ptr != 0 {
        let mut len = 0usize;
        unsafe {
            let mut p = path_ptr as *const u8;
            while *p != 0 {
                len += 1;
                p = p.add(1);
                if len > 256 {
                    return crate::syscall::types::EINVAL;
                }
            }
            let slice = core::slice::from_raw_parts(path_ptr as *const u8, len);
            if let Ok(path) = core::str::from_utf8(slice) {
                provided_path = Some(path);
            }
        }
    }
    let path = provided_path.unwrap_or("/hello.bin");

    crate::debug!("exec_kernel: path={}", path);

    if let Some(data) = crate::init::fs::read(path) {
        let entry = elf_loader::entry_point(data).unwrap_or(0);
        crate::debug!("ELF entry: {:#x}", entry);

        if let Some(eh) = elf_loader::parse_elf_header(data) {
            let phoff = eh.e_phoff as usize;
            let phentsz = eh.e_phentsize as usize;
            let phnum = eh.e_phnum as usize;
            for i in 0..phnum {
                let off_hdr = phoff + i * phentsz;
                if let Some(ph) = elf_loader::parse_phdr(data, off_hdr) {
                    if ph.p_type == elf_loader::PT_LOAD {
                        let vaddr = ph.p_vaddr;
                        let memsz = ph.p_memsz;
                        let filesz = ph.p_filesz;
                        let src_off = ph.p_offset as usize;
                        let flags = ph.p_flags;
                        let writable = (flags & 0x2) != 0;

                        crate::debug!("Mapping seg {} -> {:#x} (filesz={}, memsz={})", i, vaddr, filesz, memsz);
                        let seg_src = &data[src_off..src_off + filesz as usize];
                        if let Err(e) = crate::mem::paging::map_and_copy_segment(vaddr, filesz, memsz, seg_src, writable) {
                            crate::warn!("Failed to map segment: {:?}", e);
                            return crate::syscall::types::EINVAL;
                        }
                    }
                }
            }
        }

        // allocate small user stack near high address
        let stack_top: u64 = 0x0000_7FFF_FFF0_0000u64;
        let stack_size_pages: usize = 8; // 32KiB stack
        let stack_base = stack_top - (stack_size_pages as u64 * 4096);
        crate::debug!("Allocating user stack: base={:#x}, top={:#x}, size={} pages", 
                      stack_base, stack_top, stack_size_pages);
        if let Err(e) = crate::mem::paging::map_and_copy_segment(stack_base, 0, (stack_size_pages as u64) * 4096, &[], true) {
            crate::warn!("Failed to allocate user stack: {:?}", e);
            return crate::syscall::types::EINVAL;
        }
        crate::debug!("User stack allocated successfully");

        // Create a process and a usermode thread
        let proc = crate::task::Process::new(path, crate::task::PrivilegeLevel::User, None, 0);
        let pid = proc.id();
        if crate::task::add_process(proc).is_none() {
            return crate::syscall::types::EINVAL;
        }

        // allocate kernel stack for the new thread
        const KERNEL_THREAD_STACK_SIZE: usize = 4096 * 4;
        let kstack = match crate::task::thread::allocate_kernel_stack(KERNEL_THREAD_STACK_SIZE) {
            Some(a) => a,
            None => {
                crate::warn!("Failed to allocate kernel stack for thread");
                return crate::syscall::types::EINVAL;
            }
        };

        // ユーザーモードスレッドを作成
        let thread = crate::task::Thread::new_usermode(
            pid,
            path,
            entry,
            stack_top,
            kstack,
            KERNEL_THREAD_STACK_SIZE
        );

        if crate::task::add_thread(thread).is_none() {
            crate::warn!("Failed to add thread");
            return crate::syscall::types::EINVAL;
        }

        crate::info!("exec: created usermode process '{}' (pid={:?}, entry={:#x})", path, pid, entry);

        return pid.as_u64();
    }

    crate::syscall::types::EINVAL
}


