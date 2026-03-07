#![no_std]
#![no_main]

extern crate alloc;

mod vga_console;

use core::ptr::addr_of_mut;
use mochios::{BootInfo, MemoryRegion, MemoryType};
use uefi::prelude::*;
use uefi::proto::console::gop::GraphicsOutput;
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileInfo, FileType};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::proto::loaded_image::LoadedImage;
use uefi::table::boot::{AllocateType, MemoryType as UefiMemType, OpenProtocolAttributes, OpenProtocolParams};

/// VGA フレームバッファへ書き出す print マクロ
macro_rules! vga_print {
    ($($arg:tt)*) => {{
        let _ = core::fmt::write(&mut *vga_console::CONSOLE.lock(), format_args!($($arg)*));
    }};
}

macro_rules! vga_println {
    () => { vga_print!("\n") };
    ($($arg:tt)*) => { vga_print!("{}\n", format_args!($($arg)*)) };
}

static mut BOOT_INFO: BootInfo = BootInfo {
    physical_memory_offset: 0,
    framebuffer_addr: 0,
    framebuffer_size: 0,
    screen_width: 0,
    screen_height: 0,
    stride: 0,
    memory_map_addr: 0,
    memory_map_len: 0,
    memory_map_entry_size: 0,
    kernel_heap_addr: 0,
    initfs_addr: 0,
    initfs_size: 0,
    rootfs_addr: 0,
    rootfs_size: 0,
};

static mut MEMORY_MAP: [MemoryRegion; 256] = [MemoryRegion {
    start: 0,
    len: 0,
    region_type: MemoryType::Reserved,
}; 256];

/// ELF64 ファイルヘッダ
#[repr(C)]
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

/// ELF64 プログラムヘッダ
#[repr(C)]
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

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;

/// ELF64 動的セクションエントリ
#[repr(C)]
struct Elf64Dyn {
    d_tag: i64,
    d_val: u64,
}

/// ELF64 RELA 再配置エントリ
#[repr(C)]
struct Elf64Rela {
    r_offset: u64,
    r_info: u64,
    r_addend: i64,
}

const R_X86_64_RELATIVE: u32 = 8;
const DT_NULL: i64 = 0;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_RELAENT: i64 = 9;

/// `\System\initfs.img` を読み込んで物理アドレスとサイズを返す
unsafe fn load_initfs(bt: &BootServices, image_handle: Handle) -> (u64, usize) {
    let initfs_path = cstr16!(r"\System\initfs.img");

    // LoadedImage デバイスを優先
    let handles: alloc::vec::Vec<uefi::Handle> = if let Ok(li) = bt.open_protocol_exclusive::<LoadedImage>(image_handle) {
        if let Some(dev) = li.device() {
            drop(li);
            alloc::vec![dev]
        } else {
            bt.find_handles::<SimpleFileSystem>().unwrap_or_default()
        }
    } else {
        bt.find_handles::<SimpleFileSystem>().unwrap_or_default()
    };

    for handle in handles {
        if let Some((addr, size)) = try_load_raw(bt, image_handle, handle, initfs_path) {
            vga_println!("initfs loaded at {:#x} ({} bytes)", addr, size);
            return (addr, size);
        }
    }
    vga_println!("[WARN] initfs.img not found, initfs will be empty");
    (0, 0)
}

/// `\System\rootfs.ext2` を読み込んで物理アドレスとサイズを返す
unsafe fn load_rootfs(bt: &BootServices, image_handle: Handle) -> (u64, usize) {
    let rootfs_path = cstr16!(r"\System\rootfs.ext2");

    let handles: alloc::vec::Vec<uefi::Handle> = if let Ok(li) = bt.open_protocol_exclusive::<LoadedImage>(image_handle) {
        if let Some(dev) = li.device() {
            drop(li);
            alloc::vec![dev]
        } else {
            bt.find_handles::<SimpleFileSystem>().unwrap_or_default()
        }
    } else {
        bt.find_handles::<SimpleFileSystem>().unwrap_or_default()
    };

    for handle in handles {
        if let Some((addr, size)) = try_load_raw(bt, image_handle, handle, rootfs_path) {
            vga_println!("rootfs loaded at {:#x} ({} bytes)", addr, size);
            return (addr, size);
        }
    }
    vga_println!("[WARN] rootfs.ext2 not found");
    (0, 0)
}

/// 指定ハンドルから任意ファイルをページ単位でロードし (物理アドレス, サイズ) を返す
unsafe fn try_load_raw(
    bt: &BootServices,
    agent: uefi::Handle,
    handle: uefi::Handle,
    path: &uefi::CStr16,
) -> Option<(u64, usize)> {
    let mut sfs = bt.open_protocol::<SimpleFileSystem>(
        OpenProtocolParams { handle, agent, controller: None },
        OpenProtocolAttributes::GetProtocol,
    ).ok()?;
    let mut root = sfs.open_volume().ok()?;
    let fh = root.open(path, FileMode::Read, FileAttribute::empty()).ok()?;
    let mut file = match fh.into_type().ok()? {
        FileType::Regular(f) => f,
        _ => return None,
    };
    let mut info_buf = [0u8; 512];
    let info = file.get_info::<FileInfo>(&mut info_buf).ok()?;
    let size = info.file_size() as usize;
    if size == 0 { return None; }
    vga_println!("initfs size: {} bytes, reading...", size);
    let pages = (size + 0xFFF) / 0x1000;
    let addr = bt.allocate_pages(AllocateType::AnyPages, UefiMemType::LOADER_DATA, pages).ok()?;
    let buf = core::slice::from_raw_parts_mut(addr as *mut u8, size);
    // 大きなファイルは UEFI Read() の上限があるためチャンク単位で読む
    let mut read_total = 0usize;
    while read_total < size {
        let chunk = &mut buf[read_total..];
        match file.read(chunk) {
            Ok(0) => break, // EOF
            Ok(n) => read_total += n,
            Err(_) => return None,
        }
    }
    if read_total != size {
        vga_println!("[WARN] initfs: read {} / {} bytes", read_total, size);
    }
    Some((addr, size))
}

/// `\System\kernel.elf` を読み込み、PT_LOAD セグメントを物理アドレスに展開してエントリアドレスを返す
unsafe fn load_kernel(bt: &BootServices, image_handle: Handle) -> Option<u64> {
    let kernel_path = cstr16!(r"\System\kernel.elf");

    // LoadedImage からブートローダー自身のデバイスハンドルを取得して優先的に試みる
    match bt.open_protocol_exclusive::<LoadedImage>(image_handle) {
        Err(e) => vga_println!("LoadedImage open failed: {:?}", e.status()),
        Ok(loaded_image) => match loaded_image.device() {
            None => vga_println!("LoadedImage.device() = None"),
            Some(dev) => {
                drop(loaded_image);
                if let Some(entry) = try_load_from(bt, image_handle, dev, kernel_path) {
                    return Some(entry);
                }
                vga_println!("try_load_from (device handle) failed");
            }
        },
    }

    // フォールバック: 全 SimpleFileSystem ハンドルをスキャンして kernel.elf を探す
    match bt.find_handles::<SimpleFileSystem>() {
        Err(e) => {
            vga_println!("find_handles failed: {:?}", e.status());
            return None;
        }
        Ok(sfs_handles) => {
            vga_println!("SFS handle count: {}", sfs_handles.len());
            for handle in sfs_handles {
                if let Some(entry) = try_load_from(bt, image_handle, handle, kernel_path) {
                    return Some(entry);
                }
            }
        }
    }

    None
}

/// 指定 SFS ハンドルから kernel.elf のロードを試みる
unsafe fn try_load_from(
    bt: &BootServices,
    agent: uefi::Handle,
    handle: uefi::Handle,
    kernel_path: &uefi::CStr16,
) -> Option<u64> {
    // GetProtocol で非排他的に開く（ファームウェアが既に開いていても失敗しない）
    let mut sfs = match bt.open_protocol::<SimpleFileSystem>(
        OpenProtocolParams { handle, agent, controller: None },
        OpenProtocolAttributes::GetProtocol,
    ) {
        Ok(s) => s,
        Err(e) => {
            vga_println!("SFS open_protocol failed: {:?}", e.status());
            return None;
        }
    };
    let mut root = match sfs.open_volume() {
        Ok(r) => r,
        Err(e) => {
            vga_println!("open_volume failed: {:?}", e.status());
            return None;
        }
    };

    // カーネル ELF を開く
    let file_handle = match root.open(kernel_path, FileMode::Read, FileAttribute::empty()) {
        Ok(f) => f,
        Err(e) => {
            vga_println!("file open failed: {:?}", e.status());
            return None;
        }
    };
    let mut file = match file_handle.into_type().ok()? {
        FileType::Regular(f) => f,
        _ => {
            vga_println!("not a regular file");
            return None;
        }
    };

    // ファイルサイズを取得して一時バッファに読み込む
    let mut info_buf = [0u8; 512];
    let info = match file.get_info::<FileInfo>(&mut info_buf) {
        Ok(i) => i,
        Err(e) => {
            vga_println!("get_info failed: {:?}", e.status());
            return None;
        }
    };
    let file_size = info.file_size() as usize;
    vga_println!("kernel.elf size: {} bytes", file_size);
    let pages = (file_size + 0xFFF) / 0x1000;
    let buf_phys = match bt.allocate_pages(AllocateType::AnyPages, UefiMemType::LOADER_DATA, pages) {
        Ok(p) => p,
        Err(e) => {
            vga_println!("allocate_pages (buf) failed: {:?}", e.status());
            return None;
        }
    };
    let buf = core::slice::from_raw_parts_mut(buf_phys as *mut u8, file_size);
    match file.read(buf) {
        Ok(n) => vga_println!("read {} / {} bytes", n, file_size),
        Err(e) => {
            vga_println!("file read failed: {:?}", e.status());
            return None;
        }
    }

    // ELF マジック / クラス / アーキテクチャを検証
    let hdr = &*(buf.as_ptr() as *const Elf64Header);
    if &hdr.e_ident[0..4] != b"\x7fELF" || hdr.e_ident[4] != 2 || hdr.e_machine != 0x3E {
        vga_println!("ELF check failed: ident={:?} machine={:#x}", &hdr.e_ident[0..4], hdr.e_machine);
        return None;
    }

    // PT_LOAD セグメント全体の物理アドレス範囲を計算し、一括で確保する
    // (セグメントは隣接・重複することがあるため、個別確保は不可)
    let mut load_min = u64::MAX;
    let mut load_max = 0u64;
    for i in 0..hdr.e_phnum as usize {
        let phdr_offset = hdr.e_phoff as usize + i * hdr.e_phentsize as usize;
        let phdr = &*(buf.as_ptr().add(phdr_offset) as *const Elf64Phdr);
        if phdr.p_type != PT_LOAD || phdr.p_memsz == 0 {
            continue;
        }
        load_min = load_min.min(phdr.p_paddr & !0xFFF);
        load_max = load_max.max((phdr.p_paddr + phdr.p_memsz + 0xFFF) & !0xFFF);
    }
    if load_min == u64::MAX {
        vga_println!("no PT_LOAD segments");
        return None;
    }
    let kernel_pages = ((load_max - load_min) as usize) / 0x1000;
    vga_println!("kernel range {:#x}..{:#x} ({} pages)", load_min, load_max, kernel_pages);
    match bt.allocate_pages(AllocateType::Address(load_min), UefiMemType::LOADER_DATA, kernel_pages) {
        Ok(_) => {}
        Err(e) => {
            vga_println!("allocate_pages kernel failed: {:?}", e.status());
            return None;
        }
    }
    // 全体をゼロクリア（BSS を含む）
    core::ptr::write_bytes(load_min as *mut u8, 0, (load_max - load_min) as usize);

    // 各 PT_LOAD セグメントのデータをコピー
    for i in 0..hdr.e_phnum as usize {
        let phdr_offset = hdr.e_phoff as usize + i * hdr.e_phentsize as usize;
        let phdr = &*(buf.as_ptr().add(phdr_offset) as *const Elf64Phdr);
        if phdr.p_type != PT_LOAD || phdr.p_filesz == 0 {
            continue;
        }
        let dst = core::slice::from_raw_parts_mut(phdr.p_paddr as *mut u8, phdr.p_filesz as usize);
        let src = &buf[phdr.p_offset as usize..phdr.p_offset as usize + phdr.p_filesz as usize];
        dst.copy_from_slice(src);
    }

    // PT_DYNAMIC から RELA 再配置テーブルを探して R_X86_64_RELATIVE を適用する
    // PIE としてロードアドレス == リンクアドレス (0x200000) なので load_base = 0
    let mut rela_addr = 0u64;
    let mut rela_size = 0usize;
    let mut rela_ent = core::mem::size_of::<Elf64Rela>();
    for i in 0..hdr.e_phnum as usize {
        let phdr_offset = hdr.e_phoff as usize + i * hdr.e_phentsize as usize;
        let phdr = &*(buf.as_ptr().add(phdr_offset) as *const Elf64Phdr);
        if phdr.p_type != PT_DYNAMIC {
            continue;
        }
        let dyn_count = phdr.p_memsz as usize / core::mem::size_of::<Elf64Dyn>();
        let dyn_ptr = phdr.p_paddr as *const Elf64Dyn;
        for j in 0..dyn_count {
            let entry = &*dyn_ptr.add(j);
            match entry.d_tag {
                DT_NULL => break,
                DT_RELA => rela_addr = entry.d_val,
                DT_RELASZ => rela_size = entry.d_val as usize,
                DT_RELAENT => rela_ent = entry.d_val as usize,
                _ => {}
            }
        }
        break;
    }
    if rela_addr != 0 && rela_size > 0 && rela_ent > 0 {
        let rela_count = rela_size / rela_ent;
        vga_println!("applying {} RELA relocations", rela_count);
        for i in 0..rela_count {
            let rela = &*((rela_addr as usize + i * rela_ent) as *const Elf64Rela);
            if (rela.r_info & 0xFFFF_FFFF) as u32 == R_X86_64_RELATIVE {
                let target = rela.r_offset as *mut u64;
                *target = rela.r_addend as u64; // load_base = 0
            }
        }
    }

    Some(hdr.e_entry)
}

/// UEFI エントリーポイント
#[entry]
unsafe fn main(image_handle: Handle, mut system_table: SystemTable<Boot>) -> Status {
    if uefi::helpers::init(&mut system_table).is_err() {
        return Status::UNSUPPORTED;
    }

    // ── GOP フレームバッファを最初に取得してコンソールを初期化 ──────────────
    let (fb_addr, fb_size, screen_w, screen_h, stride) = {
        let gop_handle = match system_table
            .boot_services()
            .get_handle_for_protocol::<GraphicsOutput>()
        {
            Ok(h) => h,
            Err(_) => return Status::UNSUPPORTED,
        };
        let mut gop = match system_table
            .boot_services()
            .open_protocol_exclusive::<GraphicsOutput>(gop_handle)
        {
            Ok(g) => g,
            Err(_) => return Status::UNSUPPORTED,
        };
        let mode_info = gop.current_mode_info();
        let mut fb = gop.frame_buffer();
        let fb_ptr  = fb.as_mut_ptr() as *mut u32;
        let fb_sz   = fb.size();
        let (w, h)  = mode_info.resolution();
        let st      = mode_info.stride();
        vga_console::CONSOLE.lock().init(fb_ptr, w, h, st);
        (fb_ptr as u64, fb_sz, w, h, st)
    };

    vga_println!("mochiOS bootloader");
    vga_println!("Framebuffer: {}x{} stride={}", screen_w, screen_h, stride);

    // カーネルをロード (boot_services の借用をスコープで切る)
    let kernel_entry_addr = {
        let bt = system_table.boot_services();
        unsafe { load_kernel(bt, image_handle) }
    };
    let kernel_entry_addr = match kernel_entry_addr {
        Some(addr) => addr,
        None => {
            vga_println!("Failed to load kernel.elf");
            return Status::NOT_FOUND;
        }
    };

    // initfs を ESP から読み込む
    let (initfs_addr, initfs_size) = {
        let bt = system_table.boot_services();
        unsafe { load_initfs(bt, image_handle) }
    };

    // rootfs を ESP から読み込む
    let (rootfs_addr, rootfs_size) = {
        let bt = system_table.boot_services();
        unsafe { load_rootfs(bt, image_handle) }
    };

    // Boot Services を終了してメモリマップを取得
    let (_system_table, memory_map_iter) =
        unsafe { system_table.exit_boot_services(UefiMemType::LOADER_DATA) };

    let map_count;
    unsafe {
        let mut count = 0usize;
        for (i, desc) in memory_map_iter.entries().enumerate() {
            if i >= 256 {
                break;
            }
            MEMORY_MAP[i] = MemoryRegion {
                start: desc.phys_start,
                len: desc.page_count * 4096,
                region_type: match desc.ty {
                    UefiMemType::CONVENTIONAL => MemoryType::Usable,
                    UefiMemType::ACPI_RECLAIM => MemoryType::AcpiReclaimable,
                    UefiMemType::ACPI_NON_VOLATILE => MemoryType::AcpiNvs,
                    UefiMemType::UNUSABLE => MemoryType::BadMemory,
                    UefiMemType::LOADER_CODE | UefiMemType::LOADER_DATA => {
                        MemoryType::BootloaderReclaimable
                    }
                    _ => MemoryType::Reserved,
                },
            };
            count += 1;
        }
        map_count = count;
    }

    #[allow(static_mut_refs)]
    unsafe {
        BOOT_INFO.physical_memory_offset = 0;
        BOOT_INFO.framebuffer_addr = fb_addr;
        BOOT_INFO.framebuffer_size = fb_size;
        BOOT_INFO.screen_width = screen_w;
        BOOT_INFO.screen_height = screen_h;
        BOOT_INFO.stride = stride;
        BOOT_INFO.memory_map_addr = MEMORY_MAP.as_ptr() as u64;
        BOOT_INFO.memory_map_len = map_count;
        BOOT_INFO.memory_map_entry_size = core::mem::size_of::<MemoryRegion>();
        // kernel_heap_addr はカーネル自身が entry.rs 内で設定する
        BOOT_INFO.kernel_heap_addr = 0;
        BOOT_INFO.initfs_addr = initfs_addr;
        BOOT_INFO.initfs_size = initfs_size;
        BOOT_INFO.rootfs_addr = rootfs_addr;
        BOOT_INFO.rootfs_size = rootfs_size;
    }

    // カーネルへジャンプ (System V AMD64 ABI)
    let kernel_entry: unsafe extern "sysv64" fn(*mut BootInfo) -> ! =
        core::mem::transmute(kernel_entry_addr);
    unsafe { kernel_entry(addr_of_mut!(BOOT_INFO)) }
}

