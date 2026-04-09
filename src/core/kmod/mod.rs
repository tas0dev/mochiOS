use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::convert::TryInto;
use core::sync::atomic::{AtomicU64, Ordering};

pub mod disk;
pub mod fs;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct McxBuffer {
    pub ptr: *mut u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct McxPath {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
pub struct McxFsOps {
    pub mount: extern "C" fn(device_id: u32) -> i32,
    pub read: extern "C" fn(path: McxPath, offset: u64, buf: McxBuffer, out_read: *mut usize) -> i32,
    pub stat: extern "C" fn(path: McxPath, out_mode: *mut u16, out_size: *mut u64) -> i32,
    pub readdir: extern "C" fn(path: McxPath, buf: McxBuffer, out_len: *mut usize) -> i32,
}

pub const MODULE_MAX_READ_BYTES: usize = 64 * 1024 * 1024;

const CEXT_MAGIC: &[u8; 4] = b"MCEX";
const CEXT_FIXED_HEADER_SIZE: usize = 32;
const PT_LOAD: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_DYNSYM: u32 = 11;
const SHF_ALLOC: u64 = 0x2;
const SHT_RELA: u32 = 4;
const R_X86_64_RELATIVE: u32 = 8;
const ET_DYN: u16 = 3;
const MODULE_LOAD_BASE_START: u64 = 0x0000_6000_0000_0000;
const MODULE_LOAD_GUARD: u64 = 0x20_0000; // 2MiB guard
static NEXT_MODULE_LOAD_BASE: AtomicU64 = AtomicU64::new(MODULE_LOAD_BASE_START);

type FsInitFn = unsafe extern "C" fn() -> *const McxFsOps;
type DiskInitFn = unsafe extern "C" fn() -> *const disk::McxDiskOps;

struct CextHeader {
    module_version: u16,
    name_len: usize,
    dep_count: usize,
    header_size: usize,
    elf_size: usize,
}

struct CextMeta {
    name: String,
    deps: Vec<String>,
    module_version: u16,
    elf: Vec<u8>,
}

pub fn load_modules() {
    let Some(entries) = crate::init::fs::readdir_path("/Modules") else {
        crate::info!("kmod: /Modules is empty");
        return;
    };

    let mut module_paths: Vec<String> = entries
        .into_iter()
        .filter(|name| name.ends_with(".cext"))
        .map(|name| alloc::format!("/Modules/{}", name))
        .collect();
    module_paths.sort();

    for path in module_paths {
        let Some(bytes) = crate::init::fs::read(&path) else {
            crate::warn!("kmod: failed to read {}", path);
            continue;
        };
        let Some(meta) = parse_cext(&bytes) else {
            crate::warn!("kmod: invalid cext {}", path);
            continue;
        };

        let mut missing_dep = false;
        for dep in &meta.deps {
            if dep == "disk" && !disk::is_loaded() {
                crate::warn!("kmod: skip {} (disk not loaded)", meta.name);
                missing_dep = true;
                break;
            }
        }
        if missing_dep {
            continue;
        }

        match meta.name.as_str() {
            "disk" => {
                if let Some(addr) = load_elf_symbol(&meta.elf, "mochi_module_init") {
                    let init: DiskInitFn = unsafe { core::mem::transmute(addr) };
                    let ops = unsafe { init() };
                    if disk::register(ops, meta.module_version) {
                        crate::info!("kmod: loaded disk.cext v{}", meta.module_version);
                    } else {
                        crate::warn!("kmod: disk init returned null ops");
                    }
                } else {
                    crate::warn!("kmod: mochi_module_init not found in disk.cext");
                }
            }
            "fs" => {
                if let Some(addr) = load_elf_symbol(&meta.elf, "mochi_module_init") {
                    let init: FsInitFn = unsafe { core::mem::transmute(addr) };
                    let ops = unsafe { init() };
                    if fs::register(ops, meta.module_version) {
                        crate::info!("kmod: loaded fs.cext v{}", meta.module_version);
                    } else {
                        crate::warn!("kmod: fs init returned null ops");
                    }
                } else {
                    crate::warn!("kmod: mochi_module_init not found in fs.cext");
                }
            }
            other => {
                crate::warn!("kmod: unknown module {}", other);
            }
        }
    }
}

fn parse_cext(bytes: &[u8]) -> Option<CextMeta> {
    let header = parse_header(bytes)?;
    if header.header_size > bytes.len() {
        return None;
    }

    let mut cursor = CEXT_FIXED_HEADER_SIZE;
    let name_end = cursor.checked_add(header.name_len)?;
    let name = core::str::from_utf8(bytes.get(cursor..name_end)?).ok()?;
    cursor = name_end;

    let mut deps = Vec::with_capacity(header.dep_count);
    for _ in 0..header.dep_count {
        let dep_len = read_u16(bytes, cursor)? as usize;
        cursor = cursor.checked_add(2)?;
        let dep_end = cursor.checked_add(dep_len)?;
        let dep = core::str::from_utf8(bytes.get(cursor..dep_end)?).ok()?;
        deps.push(dep.to_string());
        cursor = dep_end;
    }
    if cursor != header.header_size {
        return None;
    }
    let elf_start = header.header_size;
    let elf_end = elf_start.checked_add(header.elf_size)?;
    if elf_end > bytes.len() {
        return None;
    }

    Some(CextMeta {
        name: name.to_string(),
        deps,
        module_version: header.module_version,
        elf: bytes[elf_start..elf_end].to_vec(),
    })
}

fn parse_header(bytes: &[u8]) -> Option<CextHeader> {
    if bytes.len() < CEXT_FIXED_HEADER_SIZE {
        return None;
    }
    if bytes.get(0..4)? != CEXT_MAGIC {
        return None;
    }
    let abi = read_u16(bytes, 4)?;
    if abi != 1 {
        return None;
    }
    let module_version = read_u16(bytes, 6)?;
    let name_len = read_u16(bytes, 8)? as usize;
    let dep_count = read_u16(bytes, 10)? as usize;
    let header_size = read_u32(bytes, 12)? as usize;
    let elf_size = read_u64(bytes, 16)? as usize;
    if header_size < CEXT_FIXED_HEADER_SIZE {
        return None;
    }

    Some(CextHeader {
        module_version,
        name_len,
        dep_count,
        header_size,
        elf_size,
    })
}

#[inline]
fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let raw = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([raw[0], raw[1]]))
}

#[inline]
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let raw = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

#[inline]
fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let raw = bytes.get(offset..offset + 8)?;
    Some(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn load_elf_symbol(elf: &[u8], symbol_name: &str) -> Option<u64> {
    let eh = crate::elf::loader::parse_elf_header(elf)?;
    let loaded = load_elf_image(elf, &eh)?;
    apply_relocations(elf, &eh, loaded.base, loaded.min_vaddr, loaded.max_vaddr)?;
    find_symbol_runtime_addr(elf, &eh, symbol_name, loaded.base, loaded.min_vaddr)
}

struct LoadedElf {
    base: u64,
    min_vaddr: u64,
    max_vaddr: u64,
}

#[inline]
fn align_up_4k(v: u64) -> Option<u64> {
    v.checked_add(0xfff).map(|x| x & !0xfff)
}

fn alloc_module_base(span: u64) -> Option<u64> {
    let size = align_up_4k(span)?;
    let step = size.checked_add(MODULE_LOAD_GUARD)?;
    let mut cur = NEXT_MODULE_LOAD_BASE.load(Ordering::Relaxed);
    loop {
        let next = cur.checked_add(step)?;
        match NEXT_MODULE_LOAD_BASE.compare_exchange(cur, next, Ordering::AcqRel, Ordering::Relaxed)
        {
            Ok(_) => return Some(cur),
            Err(actual) => cur = actual,
        }
    }
}

fn load_elf_image(elf: &[u8], eh: &crate::elf::loader::Elf64Ehdr) -> Option<LoadedElf> {
    let phoff = eh.e_phoff as usize;
    let phentsize = eh.e_phentsize as usize;
    let phnum = eh.e_phnum as usize;
    if phoff == 0 || phentsize == 0 || phnum == 0 {
        return None;
    }

    let mut min_vaddr = u64::MAX;
    let mut max_vaddr = 0u64;

    for i in 0..phnum {
        let off = phoff.checked_add(i.checked_mul(phentsize)?)?;
        let ph = crate::elf::loader::parse_phdr(elf, off)?;
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }
        min_vaddr = min_vaddr.min(ph.p_vaddr);
        max_vaddr = max_vaddr.max(ph.p_vaddr.checked_add(ph.p_memsz)?);
    }
    if min_vaddr == u64::MAX || max_vaddr <= min_vaddr {
        return None;
    }
    let is_dyn = eh.e_type == ET_DYN;
    let base = if is_dyn {
        alloc_module_base(max_vaddr.checked_sub(min_vaddr)?)?
    } else {
        0
    };
    let vaddr_bias = if is_dyn {
        base.checked_sub(min_vaddr)?
    } else {
        0
    };

    for i in 0..phnum {
        let off = phoff.checked_add(i.checked_mul(phentsize)?)?;
        let ph = crate::elf::loader::parse_phdr(elf, off)?;
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }
        let writable = (ph.p_flags & 0x2) != 0;
        let executable = (ph.p_flags & 0x1) != 0;
        let filesz = usize::try_from(ph.p_filesz).ok()?;
        let memsz = usize::try_from(ph.p_memsz).ok()?;
        if filesz > memsz {
            return None;
        }
        let src_off = usize::try_from(ph.p_offset).ok()?;
        let src_end = src_off.checked_add(filesz)?;
        if src_end > elf.len() {
            return None;
        }
        let src = &elf[src_off..src_end];
        let seg_vaddr = ph.p_vaddr.checked_add(vaddr_bias)?;
        crate::mem::paging::map_and_copy_segment(
            seg_vaddr,
            ph.p_filesz,
            ph.p_memsz,
            src,
            writable,
            executable,
        )
        .ok()?;
    }

    Some(LoadedElf {
        // ET_DYN は再配置先ベース、ET_EXEC はリンクアドレス固定運用。
        base,
        min_vaddr,
        max_vaddr,
    })
}

fn apply_relocations(
    elf: &[u8],
    eh: &crate::elf::loader::Elf64Ehdr,
    base: u64,
    min_vaddr: u64,
    max_vaddr: u64,
) -> Option<()> {
    let shoff = eh.e_shoff as usize;
    let shentsz = eh.e_shentsize as usize;
    let shnum = eh.e_shnum as usize;
    if shoff == 0 || shentsz == 0 || shnum == 0 {
        return Some(());
    }
    if shoff.checked_add(shentsz.checked_mul(shnum)?)? > elf.len() {
        return None;
    }

    for i in 0..shnum {
        let sh_off = shoff + i * shentsz;
        let sh_type = read_u32(elf, sh_off + 4)?;
        let sh_flags = read_u64(elf, sh_off + 8)?;
        if sh_type != SHT_RELA || (sh_flags & SHF_ALLOC) == 0 {
            continue;
        }
        let rela_off = usize::try_from(read_u64(elf, sh_off + 24)?).ok()?;
        let rela_size = usize::try_from(read_u64(elf, sh_off + 32)?).ok()?;
        let rela_entsize = usize::try_from(read_u64(elf, sh_off + 56)?).ok()?;
        if rela_entsize == 0 || rela_entsize < 24 || rela_size % rela_entsize != 0 {
            return None;
        }
        let rela_end = rela_off.checked_add(rela_size)?;
        if rela_end > elf.len() {
            return None;
        }
        let count = rela_size / rela_entsize;
        for r in 0..count {
            let ent = rela_off + r * rela_entsize;
            let r_offset = read_u64(elf, ent)?;
            let r_info = read_u64(elf, ent + 8)?;
            let r_addend = i64::from_le_bytes(elf.get(ent + 16..ent + 24)?.try_into().ok()?);
            let r_type = (r_info & 0xffff_ffff) as u32;
            if r_type != R_X86_64_RELATIVE {
                continue;
            }
            let reloc_vaddr = r_offset;
            if reloc_vaddr < min_vaddr || reloc_vaddr + 8 > max_vaddr {
                return None;
            }
            let dst = base.checked_add(reloc_vaddr.checked_sub(min_vaddr)?)? as *mut u64;
            let value_i128 = base as i128 + r_addend as i128 - min_vaddr as i128;
            if value_i128 < 0 || value_i128 > u64::MAX as i128 {
                return None;
            }
            let value = value_i128 as u64;
            unsafe {
                core::ptr::write_unaligned(dst, value);
            }
        }
    }

    Some(())
}

fn find_symbol_runtime_addr(
    elf: &[u8],
    eh: &crate::elf::loader::Elf64Ehdr,
    symbol_name: &str,
    base: u64,
    min_vaddr: u64,
) -> Option<u64> {
    let shoff = eh.e_shoff as usize;
    let shentsz = eh.e_shentsize as usize;
    let shnum = eh.e_shnum as usize;
    if shoff == 0 || shentsz == 0 || shnum == 0 {
        return None;
    }

    for si in 0..shnum {
        let sh_off = shoff + si * shentsz;
        let sh_type = read_u32(elf, sh_off + 4)?;
        if sh_type != SHT_SYMTAB && sh_type != SHT_DYNSYM {
            continue;
        }
        let symtab_offset = usize::try_from(read_u64(elf, sh_off + 24)?).ok()?;
        let symtab_size = usize::try_from(read_u64(elf, sh_off + 32)?).ok()?;
        let sh_link = read_u32(elf, sh_off + 40)? as usize;
        let symtab_entsize = usize::try_from(read_u64(elf, sh_off + 56)?).ok()?;
        if symtab_entsize < 24 || symtab_size == 0 {
            continue;
        }
        if sh_link >= shnum {
            continue;
        }
        let link_sh_off = shoff + sh_link * shentsz;
        let strtab_offset = usize::try_from(read_u64(elf, link_sh_off + 24)?).ok()?;
        let strtab_size = usize::try_from(read_u64(elf, link_sh_off + 32)?).ok()?;
        let nsyms = symtab_size / symtab_entsize;
        for i_sym in 0..nsyms {
            let sym_off = symtab_offset + i_sym * symtab_entsize;
            let st_name = read_u32(elf, sym_off)? as usize;
            let st_value = read_u64(elf, sym_off + 8)?;
            if st_name >= strtab_size {
                continue;
            }
            let name_off = strtab_offset + st_name;
            if name_off >= elf.len() {
                continue;
            }
            let mut end = name_off;
            while end < elf.len() && elf[end] != 0 {
                end += 1;
            }
            let Ok(name_str) = core::str::from_utf8(&elf[name_off..end]) else {
                continue;
            };
            if name_str == symbol_name {
                if base == 0 {
                    return Some(st_value);
                }
                return base.checked_add(st_value.checked_sub(min_vaddr)?);
            }
        }
    }
    None
}
