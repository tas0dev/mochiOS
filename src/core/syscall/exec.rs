use crate::elf::loader as elf_loader;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;

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

    exec_internal(path, None)
}

/// 名前を指定してカーネル内から実行可能ファイルを実行する（カーネル内部用）
pub fn exec_kernel_with_name(path: &str, name: &str) -> u64 {
    exec_internal(path, Some(name))
}

fn exec_internal(path: &str, name_override: Option<&str>) -> u64 {
    let process_name = name_override.unwrap_or(path);
    crate::debug!("exec: path={}, name={}", path, process_name);

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

        let stack_end_vaddr: u64 = 0x0000_7FFF_FFF0_0000;
        let stack_size_pages: usize = 8; // 32KiB stack
        let stack_base_vaddr = stack_end_vaddr - (stack_size_pages as u64 * 4096);

        // Prepare arguments (argv) and environment variables (envp)
        let args = [process_name];
        let envs: [&str; 0] = [];

        let mut string_block = Vec::new();
        let mut argv_offsets = Vec::new();
        for arg in args {
            argv_offsets.push(string_block.len());
            string_block.extend_from_slice(arg.as_bytes());
            string_block.push(0);
        }
        let mut envp_offsets = Vec::new();
        for env in envs {
            envp_offsets.push(string_block.len());
            string_block.extend_from_slice(env.as_bytes());
            string_block.push(0);
        }

        // Calculate layout
        let string_area_len = string_block.len();

        // Pointers: argc(8) + argv(8*N) + NULL(8) + envp(8*M) + NULL(8) + Auxv(16)
        let pointers_bytes = 8 // argc
            + (args.len() * 8) // argv
            + 8 // NULL
            + (envs.len() * 8) // envp
            + 8 // NULL
            + 16; // Auxv

        let total_data_needed = string_area_len + pointers_bytes;
        let padding_len = (16 - (total_data_needed % 16)) % 16;
        let total_size = total_data_needed + padding_len;

        let string_area_base = stack_end_vaddr - string_area_len as u64;
        let initial_rsp = stack_end_vaddr - total_size as u64;

        // Construct the top page content
        let mut page_data = Vec::new();
        let page_offset = total_size % 4096;
        let unused_space = 4096 - page_offset;

        // Fill unused space at the beginning of the page
        // (Note: if total_size > 4096, this logic needs adjustment, but args are small for now)
        if total_size > 4096 {
            crate::warn!("Arguments too large for single page stack setup");
            return crate::syscall::types::EINVAL;
        }
        page_data.resize(unused_space, 0);

        // Push Argc
        page_data.extend_from_slice(&(args.len() as u64).to_ne_bytes());

        // Push Argv Ptrs
        for off in argv_offsets {
            let ptr = string_area_base + off as u64;
            page_data.extend_from_slice(&ptr.to_ne_bytes());
        }
        // Push Argv NULL
        page_data.extend_from_slice(&0u64.to_ne_bytes());

        // Push Envp Ptrs
        for off in envp_offsets {
            let ptr = string_area_base + off as u64;
            page_data.extend_from_slice(&ptr.to_ne_bytes());
        }
        // Push Envp NULL
        page_data.extend_from_slice(&0u64.to_ne_bytes());

        // Push Auxv {0, 0}
        page_data.extend_from_slice(&0u64.to_ne_bytes());
        page_data.extend_from_slice(&0u64.to_ne_bytes());

        // Push Padding
        page_data.resize(page_data.len() + padding_len, 0);

        // Push Strings
        page_data.extend_from_slice(&string_block);

        // Verify size
        assert_eq!(page_data.len(), 4096);

        crate::debug!("Allocating user stack: base={:#x}, top={:#x}, size={} pages, rsp={:#x}",
                      stack_base_vaddr, stack_end_vaddr, stack_size_pages, initial_rsp);

        // Map the lower 7 pages as zero-filled
        if let Err(e) = crate::mem::paging::map_and_copy_segment(stack_base_vaddr, 0, (stack_size_pages - 1) as u64 * 4096, &[], true) {
             crate::warn!("Failed to allocate user stack lower: {:?}", e);
             return crate::syscall::types::EINVAL;
        }
        // Map the top page with args
        let top_page_vaddr = stack_end_vaddr - 4096;
        if let Err(e) = crate::mem::paging::map_and_copy_segment(top_page_vaddr, 4096, 4096, &page_data, true) {
             crate::warn!("Failed to allocate user stack top: {:?}", e);
             return crate::syscall::types::EINVAL;
        }

        crate::debug!("User stack allocated successfully");

        // Create a process and a usermode thread
        let proc = crate::task::Process::new(process_name, crate::task::PrivilegeLevel::User, None, 0);
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
        // RSP に initial_rsp を設定
        let thread = crate::task::Thread::new_usermode(
            pid,
            process_name,
            entry,
            initial_rsp,
            kstack,
            KERNEL_THREAD_STACK_SIZE
        );

        if crate::task::add_thread(thread).is_none() {
            crate::warn!("Failed to add thread");
            return crate::syscall::types::EINVAL;
        }

        crate::debug!("exec: created usermode process '{}' (pid={:?}, entry={:#x})", process_name, pid, entry);

        return pid.as_u64();
    }

    crate::syscall::types::EINVAL
}
