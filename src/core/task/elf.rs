//! ELFローダ

use crate::error::{KernelError, MemoryError, ProcessError, Result};
use crate::mem::{self, user, frame};
use x86_64::structures::paging::Page;
use x86_64::VirtAddr;
use x86_64::structures::paging::PageTableFlags;
use crate::task::{add_process, add_thread, Process, PrivilegeLevel, Thread};
use crate::init;

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PF_X: u32 = 0x1;
const PF_W: u32 = 0x2;

const ET_DYN: u16 = 3;

const DT_NULL: i64 = 0;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_RELAENT: i64 = 9;

const R_X86_64_RELATIVE: u32 = 8;

const PIE_LOAD_BIAS: u64 = 0x2000_0000;

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Header {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct LoadedElf {
    pub entry: u64,
    pub stack_top: u64,
    pub stack_bottom: u64,
}

pub fn load_elf(data: &[u8]) -> Result<LoadedElf> {
    let header = parse_header(data)?;
    validate_header(header)?;

    let load_bias = if header.e_type == ET_DYN { PIE_LOAD_BIAS } else { 0 };

    let phoff = header.e_phoff as usize;
    let phentsize = header.e_phentsize as usize;
    let phnum = header.e_phnum as usize;

    for i in 0..phnum {
        let off = phoff + i * phentsize;
        let phdr = read_phdr(data, off)?;
        if phdr.p_type != PT_LOAD {
            continue;
        }

        let filesz = phdr.p_filesz as usize;
        let memsz = phdr.p_memsz as usize;
        if memsz == 0 {
            continue;
        }

        let file_end = phdr.p_offset as usize + filesz;
        if file_end > data.len() {
            return Err(KernelError::Memory(MemoryError::InvalidAddress));
        }

        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        // ロード時は常に書き込み可能にする（初期データコピーのため）
        // TODO: 実行後にPF_Wに応じて保護属性を調整する
        flags |= PageTableFlags::WRITABLE;
        if phdr.p_flags & PF_X == 0 {
            flags |= PageTableFlags::NO_EXECUTE;
        }

        let vaddr = phdr.p_vaddr.wrapping_add(load_bias);
        user::map_user_range(vaddr, phdr.p_memsz, flags)?;

        unsafe {
            let dst = vaddr as *mut u8;
            let src = data.as_ptr().add(phdr.p_offset as usize);
            core::ptr::copy_nonoverlapping(src, dst, filesz);

            if memsz > filesz {
                core::ptr::write_bytes(dst.add(filesz), 0, memsz - filesz);
            }
        }
    }

    if load_bias != 0 {
        apply_relocations(data, header, load_bias)?;
    }

    let stack = user::alloc_user_stack(8)?;

    Ok(LoadedElf {
        entry: header.e_entry.wrapping_add(load_bias),
        stack_top: stack.top,
        stack_bottom: stack.bottom,
    })
}

pub fn spawn_service(path: &str, name: &'static str) -> Result<()> {
    let data = init::fs::read(path).ok_or(KernelError::InvalidParam)?;
    let loaded = load_elf(data)?;

    // Services run in Ring3 (Service), not Core
    let process = Process::new(name, PrivilegeLevel::Service, None, 1);
    let pid = process.id();

    if add_process(process).is_none() {
        return Err(KernelError::Process(ProcessError::MaxProcessesReached));
    }

    // Allocate a kernel stack (pages) for the service thread and map frames
    let stack_size = (loaded.stack_top - loaded.stack_bottom) as usize;
    let page_size: usize = 4096;
    let pages = (stack_size + page_size - 1) / page_size;

    // Allocate physical frames and map them immediately into kernel virtual space
    let first_frame = frame::allocate_frame()?;
    let first_phys = first_frame.start_address().as_u64();
    let phys_offset = crate::mem::paging::physical_memory_offset();
    let kernel_stack_addr = first_phys + phys_offset;

    // Map the first frame
    let page = Page::containing_address(VirtAddr::new(kernel_stack_addr));
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
    crate::mem::paging::map_page(page, first_frame, flags)?;

    // Allocate and map remaining frames
    for i in 1..pages {
        let f = frame::allocate_frame()?;
        let vaddr = kernel_stack_addr + (i as u64) * (page_size as u64);
        let page = Page::containing_address(VirtAddr::new(vaddr));
        crate::mem::paging::map_page(page, f, flags)?;
    }

    let entry_fn: fn() -> ! = unsafe { core::mem::transmute(loaded.entry) };
    // Create thread with kernel stack, then set its context.rsp to the user stack
    let mut thread = Thread::new(pid, name, entry_fn, kernel_stack_addr, pages * page_size);

    // Build initial user stack: argc/argv/envp/auxv and strings
    // We'll place strings at lower addresses and pointers/auxv above them.
    let mut sp = loaded.stack_top;

    // argv[0] = path
    let argv0 = path.as_bytes();
    // store argv0 string
    sp = sp.saturating_sub((argv0.len() + 1) as u64);
    unsafe {
        let dst = sp as *mut u8;
        core::ptr::copy_nonoverlapping(argv0.as_ptr(), dst, argv0.len());
        *dst.add(argv0.len()) = 0;
    }
    let argv0_addr = sp;

    // Align stack to 16 bytes
    sp &= !0xF;

    // auxv entries: (key, val) pairs
    const AT_NULL: u64 = 0;
    const AT_PHDR: u64 = 3;
    const AT_PHENT: u64 = 4;
    const AT_PHNUM: u64 = 5;
    const AT_PAGESZ: u64 = 6;
    const AT_ENTRY: u64 = 9;

    // Fetch ELF header again to compute phdr addr and counts
    let header = parse_header(data)?;
    let load_bias = if header.e_type == ET_DYN { PIE_LOAD_BIAS } else { 0 };
    let at_phdr = load_bias.wrapping_add(header.e_phoff);
    let at_phent = header.e_phentsize as u64;
    let at_phnum = header.e_phnum as u64;

    // push auxv (key,val) ... AT_NULL
    let mut push_u64 = |val: u64| {
        sp = sp.saturating_sub(8);
        unsafe { *(sp as *mut u64) = val; }
    };

    // AT_NULL
    push_u64(0);
    push_u64(AT_NULL);

    // AT_ENTRY
    push_u64(loaded.entry);
    push_u64(AT_ENTRY);

    // AT_PAGESZ
    push_u64(4096);
    push_u64(AT_PAGESZ);

    // AT_PHNUM
    push_u64(at_phnum);
    push_u64(AT_PHNUM);

    // AT_PHENT
    push_u64(at_phent);
    push_u64(AT_PHENT);

    // AT_PHDR
    push_u64(at_phdr);
    push_u64(AT_PHDR);

    // envp NULL terminator (no env)
    push_u64(0);

    // argv pointers (argv[0], NULL)
    push_u64(0); // argv NULL
    push_u64(argv0_addr);

    // argc
    push_u64(1);

    // final alignment: ensure %16 == 0
    sp &= !0xF;

    thread.context_mut().rsp = sp as u64;
    thread.context_mut().rbp = 0;

    if add_thread(thread).is_none() {
        return Err(KernelError::Process(ProcessError::MaxProcessesReached));
    }

    Ok(())
}

fn parse_header(data: &[u8]) -> Result<Elf64Header> {
    if data.len() < core::mem::size_of::<Elf64Header>() {
        return Err(KernelError::InvalidParam);
    }
    let ptr = data.as_ptr() as *const Elf64Header;
    Ok(unsafe { *ptr })
}

fn validate_header(header: Elf64Header) -> Result<()> {
    if header.e_ident[0..4] != ELF_MAGIC {
        return Err(KernelError::InvalidParam);
    }
    if header.e_ident[4] != 2 || header.e_ident[5] != 1 {
        return Err(KernelError::InvalidParam);
    }
    if header.e_phentsize as usize != core::mem::size_of::<Elf64Phdr>() {
        return Err(KernelError::InvalidParam);
    }
    Ok(())
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Dyn {
    d_tag: i64,
    d_val: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Rela {
    r_offset: u64,
    r_info: u64,
    r_addend: i64,
}

fn apply_relocations(data: &[u8], header: Elf64Header, load_bias: u64) -> Result<()> {
    let mut rela_addr = None;
    let mut rela_size = None;
    let mut rela_ent = None;

    if let Some((dyn_off, dyn_size)) = dynamic_file_range(data, header)? {
        let count = dyn_size / core::mem::size_of::<Elf64Dyn>();
        for i in 0..count {
            let off = dyn_off + i * core::mem::size_of::<Elf64Dyn>();
            let dyn_ent = read_dyn(data, off)?;
            match dyn_ent.d_tag {
                DT_NULL => break,
                DT_RELA => rela_addr = Some(dyn_ent.d_val),
                DT_RELASZ => rela_size = Some(dyn_ent.d_val as usize),
                DT_RELAENT => rela_ent = Some(dyn_ent.d_val as usize),
                _ => {}
            }
        }
    }

    let rela_addr = match rela_addr {
        Some(v) => v,
        None => return Ok(()),
    };
    let rela_size = match rela_size {
        Some(v) => v,
        None => return Ok(()),
    };
    let rela_ent = rela_ent.unwrap_or(core::mem::size_of::<Elf64Rela>());

    let rela_off = vaddr_to_offset(data, header, rela_addr)?;
    let count = rela_size / rela_ent;
    for i in 0..count {
        let off = rela_off + i * rela_ent;
        let rela = read_rela(data, off)?;
        let r_type = (rela.r_info & 0xffffffff) as u32;
        if r_type == R_X86_64_RELATIVE {
            let reloc_addr = load_bias.wrapping_add(rela.r_offset) as *mut u64;
            let value = load_bias.wrapping_add(rela.r_addend as u64);
            unsafe {
                reloc_addr.write(value);
            }
        }
    }

    Ok(())
}

fn dynamic_file_range(data: &[u8], header: Elf64Header) -> Result<Option<(usize, usize)>> {
    let phoff = header.e_phoff as usize;
    let phentsize = header.e_phentsize as usize;
    let phnum = header.e_phnum as usize;

    for i in 0..phnum {
        let off = phoff + i * phentsize;
        let phdr = read_phdr(data, off)?;
        if phdr.p_type == PT_DYNAMIC {
            return Ok(Some((phdr.p_offset as usize, phdr.p_filesz as usize)));
        }
    }

    Ok(None)
}

fn vaddr_to_offset(data: &[u8], header: Elf64Header, vaddr: u64) -> Result<usize> {
    let phoff = header.e_phoff as usize;
    let phentsize = header.e_phentsize as usize;
    let phnum = header.e_phnum as usize;

    for i in 0..phnum {
        let off = phoff + i * phentsize;
        let phdr = read_phdr(data, off)?;
        if phdr.p_type != PT_LOAD {
            continue;
        }
        let start = phdr.p_vaddr;
        let end = phdr.p_vaddr + phdr.p_memsz;
        if vaddr >= start && vaddr < end {
            let delta = vaddr - start;
            let file_off = phdr.p_offset + delta;
            if file_off as usize >= data.len() {
                return Err(KernelError::InvalidParam);
            }
            return Ok(file_off as usize);
        }
    }

    Err(KernelError::InvalidParam)
}

fn read_dyn(data: &[u8], offset: usize) -> Result<Elf64Dyn> {
    if offset + core::mem::size_of::<Elf64Dyn>() > data.len() {
        return Err(KernelError::InvalidParam);
    }
    let ptr = unsafe { data.as_ptr().add(offset) as *const Elf64Dyn };
    Ok(unsafe { *ptr })
}

fn read_rela(data: &[u8], offset: usize) -> Result<Elf64Rela> {
    if offset + core::mem::size_of::<Elf64Rela>() > data.len() {
        return Err(KernelError::InvalidParam);
    }
    let ptr = unsafe { data.as_ptr().add(offset) as *const Elf64Rela };
    Ok(unsafe { *ptr })
}

fn read_phdr(data: &[u8], offset: usize) -> Result<Elf64Phdr> {
    if offset + core::mem::size_of::<Elf64Phdr>() > data.len() {
        return Err(KernelError::InvalidParam);
    }
    let ptr = unsafe { data.as_ptr().add(offset) as *const Elf64Phdr };
    Ok(unsafe { *ptr })
}
