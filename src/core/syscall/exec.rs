use crate::elf::loader as elf_loader;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::convert::TryInto;

/// カーネル内から実行可能ファイルを読み込み実行するシステムコール
pub fn exec_kernel(path_ptr: u64) -> u64 {
    let mut provided_path: Option<&str> = None;
    if path_ptr != 0 {
        // ユーザー空間アドレスの有効性を検証する
        if !crate::syscall::validate_user_ptr(path_ptr, 256) {
            return crate::syscall::types::EINVAL;
        }
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

    // ユーザー空間からはサービス（.serviceで終わる名前）を起動できない
    // ただし core.service はサービスマネージャーとして他のサービスを起動できる
    if path.ends_with(".service") {
        let caller_is_core = crate::task::current_thread_id()
            .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
            .and_then(|pid| crate::task::with_process(pid, |p| {
                let name = p.name();
                name == "core.service" || name == "core"
            }))
            .unwrap_or(false);
        if !caller_is_core {
            return crate::syscall::types::EPERM;
        }
    }

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
        let data: &[u8] = &data;
        // MED-27修正: エントリポイントが0の場合はELFが無効として拒否する
        // 以前はentry=0のままプロセスを作成し、仮想アドレス0にジャンプしていた
        let mut entry = match elf_loader::entry_point(data) {
            Some(e) if e != 0 => e,
            _ => {
                crate::warn!("exec: ELF entry point is 0 or missing, rejecting");
                return crate::syscall::types::EINVAL;
            }
        };
        crate::debug!("ELF entry: {:#x}", entry);

        // プロセス固有のページテーブルを作成
        let new_pt_phys = match crate::mem::paging::create_user_page_table() {
            Ok(phys) => phys,
            Err(e) => {
                crate::warn!(
                    "Failed to create user page table for {}: {:?}",
                    process_name,
                    e
                );
                return crate::syscall::types::EINVAL;
            }
        };
        crate::debug!("Created user page table at {:#x}", new_pt_phys);

        // ELFアーキテクチャ検証 (MED-07)
        const EM_X86_64: u16 = 0x3E;
        if let Some(eh) = elf_loader::parse_elf_header(data) {
            if eh.e_machine != EM_X86_64 {
                crate::warn!("ELF e_machine {:#x} is not x86-64, rejecting", eh.e_machine);
                return crate::syscall::types::EINVAL;
            }
            let phoff = eh.e_phoff as usize;
            let phentsz = eh.e_phentsize as usize;
            // phentszが0の場合は無限ループを防ぐため拒否 (MED-08)
            if phentsz == 0 {
                crate::warn!("ELF phentsize is 0, rejecting");
                return crate::syscall::types::EINVAL;
            }
            let phnum = eh.e_phnum as usize;
            for i in 0..phnum {
                // オーバーフロー安全な乗算と加算 (MED-08)
                let off_hdr = match i.checked_mul(phentsz).and_then(|x| phoff.checked_add(x)) {
                    Some(o) if o < data.len() => o,
                    _ => {
                        crate::warn!("ELF program header offset overflow or out of bounds");
                        return crate::syscall::types::EINVAL;
                    }
                };
                if let Some(ph) = elf_loader::parse_phdr(data, off_hdr) {
                    if ph.p_type == elf_loader::PT_LOAD {
                        let vaddr = ph.p_vaddr;
                        let memsz = ph.p_memsz;
                        let filesz = ph.p_filesz;
                        let src_off = ph.p_offset as usize;
                        let flags = ph.p_flags;
                        let writable = (flags & 0x2) != 0;
                        let executable = (flags & 0x1) != 0;

                        // ELFセグメントのvaddrがユーザー空間内であることを検証 (CRIT-05)
                        const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;
                        if vaddr >= USER_SPACE_END {
                            crate::warn!("ELF segment vaddr {:#x} is in kernel space", vaddr);
                            return crate::syscall::types::EINVAL;
                        }
                        if memsz > 0 {
                            let vend = match vaddr.checked_add(memsz) {
                                Some(e) if e <= USER_SPACE_END => e,
                                _ => {
                                    crate::warn!("ELF segment vaddr+memsz overflows user space");
                                    return crate::syscall::types::EINVAL;
                                }
                            };
                            let _ = vend;
                        }

                        // ELFセグメントの境界チェック (CRIT-04)
                        let src_end = match src_off.checked_add(filesz as usize) {
                            Some(e) if e <= data.len() => e,
                            _ => {
                                crate::warn!("ELF segment src offset+filesz out of bounds");
                                return crate::syscall::types::EINVAL;
                            }
                        };

                        crate::debug!(
                            "Mapping seg {} -> {:#x} (filesz={}, memsz={})",
                            i,
                            vaddr,
                            filesz,
                            memsz
                        );
                        let seg_src = &data[src_off..src_end];

                        if let Err(e) = crate::mem::paging::map_and_copy_segment_to(
                            new_pt_phys,
                            vaddr,
                            filesz,
                            memsz,
                            seg_src,
                            writable,
                            executable,
                        ) {
                            crate::warn!("Failed to map segment: {:?}", e);
                            return crate::syscall::types::EINVAL;
                        }
                    }
                }
            }
        }

        let mut sinit_addr: Option<u64> = None;
        if let Some(eh_sym) = elf_loader::parse_elf_header(data) {
            let shoff = eh_sym.e_shoff as usize;
            let shentsz = eh_sym.e_shentsize as usize;
            let shnum = eh_sym.e_shnum as usize;
            if shoff > 0 && shentsz > 0 && shnum > 0 && data.len() >= shoff + shentsz * shnum {
                let mut symtab_offset: usize = 0;
                let mut symtab_size: usize = 0;
                let mut symtab_entsize: usize = 0;
                let mut strtab_offset: usize = 0;
                let mut strtab_size: usize = 0;
                for si in 0..shnum {
                    let sh_off = shoff + si * shentsz;
                    if sh_off + shentsz > data.len() {
                        break;
                    }
                    let sh_type = match data[sh_off + 4..sh_off + 8].try_into() {
                        Ok(b) => u32::from_le_bytes(b),
                        Err(_) => {
                            crate::warn!("ELF section header truncated");
                            return crate::syscall::types::EINVAL;
                        }
                    };
                    let sh_offset = match data[sh_off + 24..sh_off + 32].try_into() {
                        Ok(b) => u64::from_le_bytes(b) as usize,
                        Err(_) => {
                            crate::warn!("ELF section header truncated");
                            return crate::syscall::types::EINVAL;
                        }
                    };
                    let sh_size = match data[sh_off + 32..sh_off + 40].try_into() {
                        Ok(b) => u64::from_le_bytes(b) as usize,
                        Err(_) => {
                            crate::warn!("ELF section header truncated");
                            return crate::syscall::types::EINVAL;
                        }
                    };
                    let sh_link = match data[sh_off + 40..sh_off + 44].try_into() {
                        Ok(b) => u32::from_le_bytes(b),
                        Err(_) => {
                            crate::warn!("ELF section header truncated");
                            return crate::syscall::types::EINVAL;
                        }
                    };
                    let sh_entsize = match data[sh_off + 56..sh_off + 64].try_into() {
                        Ok(b) => u64::from_le_bytes(b) as usize,
                        Err(_) => {
                            crate::warn!("ELF section header truncated");
                            return crate::syscall::types::EINVAL;
                        }
                    };

                    // SHT_SYMTAB == 2
                    if sh_type == 2 {
                        symtab_offset = sh_offset;
                        symtab_size = sh_size;
                        symtab_entsize = sh_entsize;
                        // linked string table
                        let link_idx = sh_link as usize;
                        if link_idx < shnum {
                            let link_sh_off = shoff + link_idx * shentsz;
                            strtab_offset =
                                match data[link_sh_off + 24..link_sh_off + 32].try_into() {
                                    Ok(b) => u64::from_le_bytes(b) as usize,
                                    Err(_) => {
                                        crate::warn!("ELF section header truncated");
                                        return crate::syscall::types::EINVAL;
                                    }
                                };
                            strtab_size = match data[link_sh_off + 32..link_sh_off + 40].try_into()
                            {
                                Ok(b) => u64::from_le_bytes(b) as usize,
                                Err(_) => {
                                    crate::warn!("ELF section header truncated");
                                    return crate::syscall::types::EINVAL;
                                }
                            };
                        }
                        break;
                    }
                }
                if symtab_offset > 0 && strtab_offset > 0 && symtab_entsize > 0 {
                    let nsyms = symtab_size / symtab_entsize;
                    for i_sym in 0..nsyms {
                        let sym_off = symtab_offset + i_sym * symtab_entsize;
                        if sym_off + symtab_entsize > data.len() {
                            break;
                        }
                        let st_name = match data[sym_off..sym_off + 4].try_into() {
                            Ok(b) => u32::from_le_bytes(b) as usize,
                            Err(_) => {
                                crate::warn!("ELF symbol entry truncated");
                                break;
                            }
                        };
                        let st_value = match data[sym_off + 8..sym_off + 16].try_into() {
                            Ok(b) => u64::from_le_bytes(b),
                            Err(_) => {
                                crate::warn!("ELF symbol entry truncated");
                                break;
                            }
                        };

                        if st_name < strtab_size {
                            let name_off = strtab_offset + st_name;
                            if name_off < data.len() {
                                let mut end = name_off;
                                while end < data.len() && data[end] != 0 {
                                    end += 1;
                                }
                                if end <= data.len() {
                                    if let Ok(name_str) = core::str::from_utf8(&data[name_off..end])
                                    {
                                        if name_str == "__sinit" {
                                            sinit_addr = Some(st_value);
                                            break;
                                        }
                                    }
                                }
                            }
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

        // スタックのトップページにバッファを配置
        let mut page_data = Vec::new();
        let page_offset = total_size % 4096;
        let unused_space = 4096 - page_offset;

        // 使用する引数と環境変数のサイズを確認
        // 4096バイトのページに収まらない場合はエラー
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

        // サイズを確認
        if page_data.len() != 4096 {
            crate::warn!("internal: page_data.len() != 4096: {}", page_data.len());
            return crate::syscall::types::EINVAL;
        }

        crate::debug!(
            "Allocating user stack: base={:#x}, top={:#x}, size={} pages, rsp={:#x}",
            stack_base_vaddr,
            stack_end_vaddr,
            stack_size_pages,
            initial_rsp
        );

        // Map the lower 7 pages as zero-filled (writable, non-executable stack)
        if let Err(e) = crate::mem::paging::map_and_copy_segment_to(
            new_pt_phys,
            stack_base_vaddr,
            0,
            (stack_size_pages - 1) as u64 * 4096,
            &[],
            true,
            false,
        ) {
            crate::warn!("Failed to allocate user stack lower: {:?}", e);
            return crate::syscall::types::EINVAL;
        }
        // Map the top page with args (writable, non-executable stack)
        let top_page_vaddr = stack_end_vaddr - 4096;
        if let Err(e) = crate::mem::paging::map_and_copy_segment_to(
            new_pt_phys,
            top_page_vaddr,
            4096,
            4096,
            &page_data,
            true,
            false,
        ) {
            crate::warn!("Failed to allocate user stack top: {:?}", e);
            return crate::syscall::types::EINVAL;
        }

        crate::debug!("User stack allocated successfully");

        // Pre-map initial heap pages to avoid immediate page faults from user allocations.
        // Map two pages at the default heap base so small early allocations won't fault.
        let default_heap_base: u64 = 0x4000_0000;
        let heap_map_size: u64 = 4096 * 2;
        if let Err(e) = crate::mem::paging::map_and_copy_segment_to(
            new_pt_phys,
            default_heap_base,
            0,
            heap_map_size,
            &[],
            true,
            false,
        ) {
            crate::warn!(
                "Failed to pre-map initial heap pages at {:#x}: {:?}",
                default_heap_base,
                e
            );
        } else {
            crate::info!(
                "Pre-mapped {} bytes for heap at {:#x} for {}",
                heap_map_size,
                default_heap_base,
                process_name
            );
        }

        // __sinitがあれば、スタブを作成して先に呼び出す
        if let Some(sinit) = sinit_addr {
            let stub_addr: u64 = default_heap_base + heap_map_size;
            crate::info!(
                "Found __sinit at {:#x}, mapping init stub at {:#x}",
                sinit,
                stub_addr
            );
            let mut stub_page = vec![0u8; 4096];
            let mut cur = 0usize;
            if cur + 24 > stub_page.len() {
                crate::warn!("__sinit stub size overflow: {}", cur + 24);
                return crate::syscall::types::EINVAL;
            }
            // movabs rax, <sinit>
            stub_page[cur..cur + 2].copy_from_slice(&[0x48, 0xB8]);
            cur += 2;
            stub_page[cur..cur + 8].copy_from_slice(&sinit.to_le_bytes());
            cur += 8;
            // call rax
            stub_page[cur..cur + 2].copy_from_slice(&[0xFF, 0xD0]);
            cur += 2;
            // movabs rax, <entry>
            stub_page[cur..cur + 2].copy_from_slice(&[0x48, 0xB8]);
            cur += 2;
            stub_page[cur..cur + 8].copy_from_slice(&entry.to_le_bytes());
            cur += 8;
            // jmp rax
            stub_page[cur..cur + 2].copy_from_slice(&[0xFF, 0xE0]);
            cur += 2;

            if let Err(e) = crate::mem::paging::map_and_copy_segment_to(
                new_pt_phys,
                stub_addr,
                cur as u64,
                4096,
                &stub_page[0..cur],
                false,
                true,
            ) {
                crate::warn!("Failed to map __sinit stub at {:#x}: {:?}", stub_addr, e);
            } else {
                // jump to stub first
                entry = stub_addr;
            }
        }

        // プロセスを作成してページテーブルをセット
        let mut proc =
            crate::task::Process::new(process_name, crate::task::PrivilegeLevel::User, None, 0);
        proc.set_page_table(new_pt_phys);
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
            KERNEL_THREAD_STACK_SIZE,
        );

        crate::info!(
            "exec: loaded '{}', entry={:#x}, pid={:?}",
            process_name,
            entry,
            pid
        );

        if crate::task::add_thread(thread).is_none() {
            crate::warn!("Failed to add thread");
            return crate::syscall::types::EINVAL;
        }

        crate::debug!(
            "exec: created usermode process '{}' (pid={:?}, entry={:#x})",
            process_name,
            pid,
            entry
        );

        return pid.as_u64();
    }

    crate::syscall::types::EINVAL
}

/// execve システムコール
///
/// 現在のプロセスイメージを新しいプログラムで置き換える
///
/// # 引数
/// - `path_ptr`: 実行ファイルパスのポインタ (null 終端)
/// - `_argv`: 引数ベクタ (現在は無視)
/// - `_envp`: 環境変数ベクタ (現在は無視)
pub fn execve_syscall(path_ptr: u64, _argv: u64, _envp: u64) -> u64 {
    use crate::syscall::types::{EINVAL, ENOENT, EPERM};

    if path_ptr == 0 {
        return EINVAL;
    }

    // ユーザー空間アドレスの有効性を検証する
    if !crate::syscall::validate_user_ptr(path_ptr, 256) {
        return EINVAL;
    }

    // ユーザー空間から null 終端パスを読み込む
    let mut len = 0usize;
    unsafe {
        let mut p = path_ptr as *const u8;
        while *p != 0 {
            len += 1;
            p = p.add(1);
            if len > 256 {
                return EINVAL;
            }
        }
    }
    let path_bytes = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, len) };
    let path = match core::str::from_utf8(path_bytes) {
        Ok(s) => s,
        Err(_) => return EINVAL,
    };

    // ユーザー空間からはサービス（.serviceで終わる名前）を起動できない
    if path.ends_with(".service") {
        return EPERM;
    }

    // initfs からファイルを読み込む
    let data_vec = match crate::init::fs::read(path) {
        Some(d) => d,
        None => return ENOENT,
    };
    let data: &[u8] = &data_vec;

    // ELF エントリポイントとセグメントを解析
    let entry = match crate::elf::loader::entry_point(data) {
        Some(e) => e,
        None => return EINVAL,
    };

    // 新しいページテーブルを作成
    let new_pt_phys = match crate::mem::paging::create_user_page_table() {
        Ok(p) => p,
        Err(_) => return EINVAL,
    };

    // PT_LOAD セグメントをマップ
    const EM_X86_64_EXECVE: u16 = 0x3E;
    const USER_SPACE_END_EXECVE: u64 = 0x0000_7FFF_FFFF_FFFF;
    if let Some(eh) = crate::elf::loader::parse_elf_header(data) {
        // ELFアーキテクチャ検証 (MED-07)
        if eh.e_machine != EM_X86_64_EXECVE {
            crate::warn!("execve: ELF e_machine {:#x} is not x86-64", eh.e_machine);
            return EINVAL;
        }
        let phoff = eh.e_phoff as usize;
        let phentsz = eh.e_phentsize as usize;
        // phentszが0の場合は無限ループを防ぐ (MED-08)
        if phentsz == 0 {
            return EINVAL;
        }
        let phnum = eh.e_phnum as usize;
        for i in 0..phnum {
            // オーバーフロー安全な乗算と加算 (MED-08)
            let off_hdr = match i.checked_mul(phentsz).and_then(|x| phoff.checked_add(x)) {
                Some(o) if o < data.len() => o,
                _ => return EINVAL,
            };
            if let Some(ph) = crate::elf::loader::parse_phdr(data, off_hdr) {
                if ph.p_type == crate::elf::loader::PT_LOAD {
                    // ELFセグメントのvaddrがユーザー空間内であることを検証 (CRIT-05)
                    if ph.p_vaddr >= USER_SPACE_END_EXECVE {
                        crate::warn!("execve: ELF segment vaddr {:#x} is in kernel space", ph.p_vaddr);
                        return EINVAL;
                    }
                    if ph.p_memsz > 0 {
                        match ph.p_vaddr.checked_add(ph.p_memsz) {
                            Some(e) if e <= USER_SPACE_END_EXECVE => {}
                            _ => {
                                crate::warn!("execve: ELF segment vaddr+memsz overflows user space");
                                return EINVAL;
                            }
                        }
                    }
                    // ELFセグメントの境界チェック (CRIT-04)
                    let src_off = ph.p_offset as usize;
                    let src_end = match src_off.checked_add(ph.p_filesz as usize) {
                        Some(e) if e <= data.len() => e,
                        _ => {
                            crate::warn!("execve: ELF segment src offset+filesz out of bounds");
                            return EINVAL;
                        }
                    };
                    let seg_src = &data[src_off..src_end];
                    if let Err(_) = crate::mem::paging::map_and_copy_segment_to(
                        new_pt_phys,
                        ph.p_vaddr,
                        ph.p_filesz,
                        ph.p_memsz,
                        seg_src,
                        (ph.p_flags & 0x2) != 0,
                        (ph.p_flags & 0x1) != 0,
                    ) {
                        return EINVAL;
                    }
                }
            }
        }
    }

    // ユーザースタックをセットアップ (exec_internal と同じレイアウト)
    let stack_end_vaddr: u64 = 0x0000_7FFF_FFF0_0000;
    let stack_size_pages: usize = 8;
    let stack_base_vaddr = stack_end_vaddr - (stack_size_pages as u64 * 4096);

    let args = [path];
    let mut string_block: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut argv_offsets: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    for arg in args {
        argv_offsets.push(string_block.len());
        string_block.extend_from_slice(arg.as_bytes());
        string_block.push(0);
    }
    let string_area_len = string_block.len();
    let pointers_bytes = 8 + (args.len() * 8) + 8 + 8 + 16;
    let total_data_needed = string_area_len + pointers_bytes;
    let padding_len = (16 - (total_data_needed % 16)) % 16;
    let total_size = total_data_needed + padding_len;
    if total_size > 4096 {
        return EINVAL;
    }
    let string_area_base = stack_end_vaddr - string_area_len as u64;
    let initial_rsp = stack_end_vaddr - total_size as u64;

    let mut page_data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let page_offset = total_size % 4096;
    let unused_space = 4096 - page_offset;
    page_data.resize(unused_space, 0);
    page_data.extend_from_slice(&(args.len() as u64).to_ne_bytes());
    for off in argv_offsets {
        page_data.extend_from_slice(&(string_area_base + off as u64).to_ne_bytes());
    }
    page_data.extend_from_slice(&0u64.to_ne_bytes()); // argv null
    page_data.extend_from_slice(&0u64.to_ne_bytes()); // envp null
    page_data.extend_from_slice(&0u64.to_ne_bytes()); // auxv[0]
    page_data.extend_from_slice(&0u64.to_ne_bytes()); // auxv[1]
    page_data.resize(page_data.len() + padding_len, 0);
    page_data.extend_from_slice(&string_block);
    if page_data.len() != 4096 {
        crate::warn!("internal: page_data.len() != 4096: {}", page_data.len());
        return crate::syscall::types::EINVAL;
    }

    if let Err(_) = crate::mem::paging::map_and_copy_segment_to(
        new_pt_phys,
        stack_base_vaddr,
        0,
        (stack_size_pages - 1) as u64 * 4096,
        &[],
        true,
        false,
    ) {
        return EINVAL;
    }
    let top_page_vaddr = stack_end_vaddr - 4096;
    if let Err(_) = crate::mem::paging::map_and_copy_segment_to(
        new_pt_phys,
        top_page_vaddr,
        4096,
        4096,
        &page_data,
        true,
        false,
    ) {
        return EINVAL;
    }

    // 現在のプロセスのページテーブルとヒープを更新
    let current_tid = match crate::task::current_thread_id() {
        Some(t) => t,
        None => return EINVAL,
    };
    let pid = match crate::task::with_thread(current_tid, |t| t.process_id()) {
        Some(p) => p,
        None => return EINVAL,
    };
    crate::task::with_process_mut(pid, |p| {
        p.set_page_table(new_pt_phys);
        p.set_heap_start(0);
        p.set_heap_end(0);
    });

    // 新しいページテーブルに切り替えてジャンプ
    unsafe {
        crate::mem::paging::switch_page_table(new_pt_phys);
        crate::task::jump_to_usermode(entry, initial_rsp);
    }
}
