//! ELFローダ

use crate::init;
use crate::mem::{paging, user};
use crate::result::{Kernel, Memory, Process, Result};
use crate::task::{
    add_process, add_thread, remove_process, PrivilegeLevel, Process as TaskProcess, Thread,
};
use core::sync::atomic::{AtomicU64, Ordering};

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PF_X: u32 = 0x1;
const PF_W: u32 = 0x2;

const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 0x3E;

const DT_NULL: i64 = 0;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_RELAENT: i64 = 9;

const R_X86_64_RELATIVE: u32 = 8;

const PIE_LOAD_BIAS: u64 = 0x2000_0000;
const PIE_ASLR_WINDOW_PAGES: u64 = 0x4000; // 64MiB
static PIE_ASLR_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    pub load_bias: u64,
    pub stack_top: u64,
    pub stack_bottom: u64,
}

struct ServiceSpawnGuard {
    pid: crate::task::ProcessId,
    page_table: u64,
    kernel_stack: Option<u64>,
    disarmed: bool,
}

impl ServiceSpawnGuard {
    fn new(pid: crate::task::ProcessId, page_table: u64) -> Self {
        Self {
            pid,
            page_table,
            kernel_stack: None,
            disarmed: false,
        }
    }

    fn set_kernel_stack(&mut self, kernel_stack: u64) {
        self.kernel_stack = Some(kernel_stack);
    }

    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for ServiceSpawnGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        if let Some(stack) = self.kernel_stack {
            crate::task::free_kernel_stack(stack);
        }
        let _ = remove_process(self.pid);
        let _ = paging::destroy_user_page_table(self.page_table);
    }
}

#[inline]
fn aslr_mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn next_pie_load_bias() -> u64 {
    if PIE_ASLR_COUNTER.load(Ordering::Relaxed) == 0 {
        let mut init = crate::cpu::boot_entropy_u64() ^ 0xa726_f38d_c941_5e2b;
        if init == 0 {
            init = 1;
        }
        let _ = PIE_ASLR_COUNTER.compare_exchange(0, init, Ordering::SeqCst, Ordering::Relaxed);
    }
    let ctr = PIE_ASLR_COUNTER.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed);
    let ticks = crate::interrupt::timer::get_ticks();
    let hw = crate::cpu::hw_random_u64().unwrap_or(0);
    let boot = crate::cpu::boot_entropy_u64();
    let offset_pages =
        aslr_mix64(ctr ^ ticks.rotate_left(13) ^ hw.rotate_left(11) ^ boot) % PIE_ASLR_WINDOW_PAGES;
    PIE_LOAD_BIAS + offset_pages * 4096
}

fn current_user_page_table() -> Result<u64> {
    let pid = crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |thread| thread.process_id()))
        .ok_or(Kernel::Memory(Memory::NotMapped))?;
    crate::task::with_process(pid, |proc| proc.page_table())
        .flatten()
        .ok_or(Kernel::Memory(Memory::NotMapped))
}

fn write_user_bytes_in_table(table_phys: u64, user_addr: u64, bytes: &[u8]) -> Result<()> {
    paging::copy_to_user_in_table(table_phys, user_addr, bytes)
}

fn write_user_u64_in_table(table_phys: u64, user_addr: u64, value: u64) -> Result<()> {
    write_user_bytes_in_table(table_phys, user_addr, &value.to_ne_bytes())
}

pub fn load_elf(data: &[u8]) -> Result<LoadedElf> {
    let table_phys = current_user_page_table()?;
    load_elf_into(table_phys, data)
}

pub fn load_elf_into(table_phys: u64, data: &[u8]) -> Result<LoadedElf> {
    let header = parse_header(data)?;
    validate_header(header)?;

    let load_bias = if header.e_type == ET_DYN {
        next_pie_load_bias()
    } else {
        0
    };

    let phoff = header.e_phoff as usize;
    let phentsize = header.e_phentsize as usize;
    let phnum = header.e_phnum as usize;

    for i in 0..phnum {
        let off = match i.checked_mul(phentsize).and_then(|x| phoff.checked_add(x)) {
            Some(o) => o,
            None => return Err(Kernel::InvalidParam),
        };
        let phdr = read_phdr(data, off)?;
        if phdr.p_type != PT_LOAD {
            continue;
        }

        let filesz = phdr.p_filesz as usize;
        let memsz = phdr.p_memsz as usize;
        if memsz == 0 {
            continue;
        }
        if filesz > memsz {
            return Err(Kernel::Memory(Memory::InvalidAddress));
        }

        let file_end = match (phdr.p_offset as usize).checked_add(filesz) {
            Some(v) => v,
            None => return Err(Kernel::Memory(Memory::InvalidAddress)),
        };
        if file_end > data.len() {
            return Err(Kernel::Memory(Memory::InvalidAddress));
        }

        let vaddr = phdr.p_vaddr.wrapping_add(load_bias);
        let seg_src = &data[phdr.p_offset as usize..file_end];
        let writable = (phdr.p_flags & PF_W) != 0;
        let executable = (phdr.p_flags & PF_X) != 0;
        if writable && executable {
            return Err(Kernel::Memory(Memory::PermissionDenied));
        }
        paging::map_and_copy_segment_to(
            table_phys,
            vaddr,
            filesz as u64,
            phdr.p_memsz,
            seg_src,
            writable,
            executable,
        )?;
    }

    if load_bias != 0 {
        apply_relocations_to(table_phys, data, header, load_bias)?;
    }

    let stack = user::alloc_user_stack_in_table(table_phys, 8)?;

    Ok(LoadedElf {
        entry: header.e_entry.wrapping_add(load_bias),
        load_bias,
        stack_top: stack.top,
        stack_bottom: stack.bottom,
    })
}

pub fn spawn_service(path: &str, name: &'static str) -> Result<()> {
    let data = init::fs::read(path).ok_or(Kernel::InvalidParam)?;
    let new_pt_phys = paging::create_user_page_table()?;

    let mut process = TaskProcess::new(name, PrivilegeLevel::Service, None, 1);
    process.set_page_table(new_pt_phys);
    let pid = process.id();

    if add_process(process).is_none() {
        let _ = paging::destroy_user_page_table(new_pt_phys);
        return Err(Kernel::Process(Process::MaxProcessesReached));
    }
    let mut guard = ServiceSpawnGuard::new(pid, new_pt_phys);

    let loaded = load_elf_into(new_pt_phys, &data)?;

    let stack_size = (loaded.stack_top - loaded.stack_bottom) as usize;
    let kernel_stack_size = stack_size
        .checked_add(4095)
        .map(|v| v & !4095usize)
        .ok_or(Kernel::Memory(Memory::OutOfMemory))?;
    let kernel_stack_addr = crate::task::allocate_kernel_stack(kernel_stack_size)
        .ok_or(Kernel::Memory(Memory::OutOfMemory))?;
    guard.set_kernel_stack(kernel_stack_addr);

    let mut thread = Thread::new_usermode(
        pid,
        name,
        loaded.entry,
        loaded.stack_top,
        kernel_stack_addr,
        kernel_stack_size,
    );

    // Build initial user stack: argc/argv/envp/auxv and strings
    // We'll place strings at lower addresses and pointers/auxv above them.
    let mut sp = loaded.stack_top;

    // argv[0] = path
    let argv0 = path.as_bytes();
    // store argv0 string
    sp = sp.saturating_sub((argv0.len() + 1) as u64);
    if let Err(err) = write_user_bytes_in_table(new_pt_phys, sp, argv0) {
        let _ = remove_process(pid);
        let _ = paging::destroy_user_page_table(new_pt_phys);
        return Err(err);
    }
    if let Err(err) = write_user_bytes_in_table(new_pt_phys, sp + argv0.len() as u64, &[0]) {
        let _ = remove_process(pid);
        let _ = paging::destroy_user_page_table(new_pt_phys);
        return Err(err);
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
    let header = parse_header(&data)?;
    let load_bias = loaded.load_bias;
    let at_phdr = load_bias.wrapping_add(header.e_phoff);
    let at_phent = header.e_phentsize as u64;
    let at_phnum = header.e_phnum as u64;

    // push auxv (key,val) ... AT_NULL
    let mut push_u64 = |val: u64| -> Result<()> {
        let new_sp = sp
            .checked_sub(8)
            .ok_or(Kernel::Memory(Memory::InvalidAddress))?;
        if new_sp < loaded.stack_bottom {
            return Err(Kernel::Memory(Memory::InvalidAddress));
        }
        sp = new_sp;
        write_user_u64_in_table(new_pt_phys, sp, val)
    };

    // AT_NULL
    push_u64(0)?;
    push_u64(AT_NULL)?;

    // AT_ENTRY
    push_u64(loaded.entry)?;
    push_u64(AT_ENTRY)?;

    // AT_PAGESZ
    push_u64(4096)?;
    push_u64(AT_PAGESZ)?;

    // AT_PHNUM
    push_u64(at_phnum)?;
    push_u64(AT_PHNUM)?;

    // AT_PHENT
    push_u64(at_phent)?;
    push_u64(AT_PHENT)?;

    // AT_PHDR
    push_u64(at_phdr)?;
    push_u64(AT_PHDR)?;

    // envp NULL terminator (no env)
    push_u64(0)?;

    // argv pointers (argv[0], NULL)
    push_u64(0)?; // argv NULL
    push_u64(argv0_addr)?;

    // argc
    push_u64(1)?;

    // final alignment: ensure %16 == 0
    sp &= !0xF;

    thread.context_mut().rsp = sp;
    thread.context_mut().rbp = 0;

    if add_thread(thread).is_none() {
        return Err(Kernel::Process(Process::MaxProcessesReached));
    }

    guard.disarm();
    Ok(())
}

fn parse_header(data: &[u8]) -> Result<Elf64Header> {
    if data.len() < core::mem::size_of::<Elf64Header>() {
        return Err(Kernel::InvalidParam);
    }
    let ptr = data.as_ptr() as *const Elf64Header;
    Ok(unsafe { *ptr })
}

fn validate_header(header: Elf64Header) -> Result<()> {
    if header.e_ident[0..4] != ELF_MAGIC {
        return Err(Kernel::InvalidParam);
    }
    if header.e_ident[4] != 2 || header.e_ident[5] != 1 {
        return Err(Kernel::InvalidParam);
    }
    if header.e_machine != EM_X86_64 {
        return Err(Kernel::InvalidParam);
    }
    if header.e_phentsize as usize != core::mem::size_of::<Elf64Phdr>() {
        return Err(Kernel::InvalidParam);
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

fn apply_relocations_to(
    table_phys: u64,
    data: &[u8],
    header: Elf64Header,
    load_bias: u64,
) -> Result<()> {
    let mut rela_addr = None;
    let mut rela_size = None;
    let mut rela_ent = None;
    let mut load_ranges: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();

    let phoff = header.e_phoff as usize;
    let phentsize = header.e_phentsize as usize;
    let phnum = header.e_phnum as usize;
    for i in 0..phnum {
        let off = match i.checked_mul(phentsize).and_then(|x| phoff.checked_add(x)) {
            Some(o) => o,
            None => return Err(Kernel::InvalidParam),
        };
        let phdr = read_phdr(data, off)?;
        if phdr.p_type != PT_LOAD || phdr.p_memsz == 0 {
            continue;
        }
        let start = phdr.p_vaddr.wrapping_add(load_bias);
        let end = match start.checked_add(phdr.p_memsz) {
            Some(v) => v,
            None => return Err(Kernel::InvalidParam),
        };
        load_ranges.push((start, end));
    }

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
    if rela_ent < core::mem::size_of::<Elf64Rela>() || rela_size % rela_ent != 0 {
        return Err(Kernel::InvalidParam);
    }

    let rela_off = vaddr_to_offset(data, header, rela_addr)?;
    let count = rela_size / rela_ent;
    for i in 0..count {
        let off = rela_off + i * rela_ent;
        let rela = read_rela(data, off)?;
        let r_type = (rela.r_info & 0xffffffff) as u32;
        if r_type == R_X86_64_RELATIVE {
            let reloc_vaddr = load_bias.wrapping_add(rela.r_offset);
            let reloc_end = match reloc_vaddr.checked_add(core::mem::size_of::<u64>() as u64) {
                Some(v) => v,
                None => return Err(Kernel::InvalidParam),
            };
            if !load_ranges
                .iter()
                .any(|(start, end)| reloc_vaddr >= *start && reloc_end <= *end)
            {
                return Err(Kernel::InvalidParam);
            }
            let value = load_bias.wrapping_add(rela.r_addend as u64);
            write_user_u64_in_table(table_phys, reloc_vaddr, value)?;
        }
    }

    Ok(())
}

fn dynamic_file_range(data: &[u8], header: Elf64Header) -> Result<Option<(usize, usize)>> {
    let phoff = header.e_phoff as usize;
    let phentsize = header.e_phentsize as usize;
    let phnum = header.e_phnum as usize;

    for i in 0..phnum {
        let off = match i.checked_mul(phentsize).and_then(|x| phoff.checked_add(x)) {
            Some(o) => o,
            None => return Err(Kernel::InvalidParam),
        };
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
        let off = match i.checked_mul(phentsize).and_then(|x| phoff.checked_add(x)) {
            Some(o) => o,
            None => return Err(Kernel::InvalidParam),
        };
        let phdr = read_phdr(data, off)?;
        if phdr.p_type != PT_LOAD {
            continue;
        }
        let start = phdr.p_vaddr;
        let end = match phdr.p_vaddr.checked_add(phdr.p_memsz) {
            Some(v) => v,
            None => return Err(Kernel::InvalidParam),
        };
        if vaddr >= start && vaddr < end {
            let delta = vaddr - start;
            let file_off = phdr.p_offset + delta;
            if file_off as usize >= data.len() {
                return Err(Kernel::InvalidParam);
            }
            return Ok(file_off as usize);
        }
    }

    Err(Kernel::InvalidParam)
}

fn read_dyn(data: &[u8], offset: usize) -> Result<Elf64Dyn> {
    if offset + core::mem::size_of::<Elf64Dyn>() > data.len() {
        return Err(Kernel::InvalidParam);
    }
    let ptr = unsafe { data.as_ptr().add(offset) as *const Elf64Dyn };
    Ok(unsafe { *ptr })
}

fn read_rela(data: &[u8], offset: usize) -> Result<Elf64Rela> {
    if offset + core::mem::size_of::<Elf64Rela>() > data.len() {
        return Err(Kernel::InvalidParam);
    }
    let ptr = unsafe { data.as_ptr().add(offset) as *const Elf64Rela };
    Ok(unsafe { *ptr })
}

fn read_phdr(data: &[u8], offset: usize) -> Result<Elf64Phdr> {
    if offset + core::mem::size_of::<Elf64Phdr>() > data.len() {
        return Err(Kernel::InvalidParam);
    }
    let ptr = unsafe { data.as_ptr().add(offset) as *const Elf64Phdr };
    Ok(unsafe { *ptr })
}
