use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let initfs_dir_core = manifest_dir.join("src/initfs");
    let initfs_dir_legacy = manifest_dir.join("src/init/initfs");
    let initfs_dir = if initfs_dir_core.is_dir() {
        initfs_dir_core
    } else {
        initfs_dir_legacy
    };

    if !initfs_dir.is_dir() {
        panic!("initfs directory not found at {:?}", initfs_dir);
    }

    // Ensure a small test ELF exists in initfs: hello.bin
    let hello_path = initfs_dir.join("hello.bin");
    if !hello_path.exists() {
        // craft a minimal ELF64 executable with one PT_LOAD containing a few bytes (hlt; jmp .)
        let mut elf: Vec<u8> = Vec::new();
        // e_ident (16)
        elf.extend(&[0x7f, b'E', b'L', b'F']);
        elf.push(2); // ELFCLASS64
        elf.push(1); // little endian
        elf.push(1); // version
        elf.extend(&[0u8; 9]);
        // e_type, e_machine, e_version
        elf.extend(&2u16.to_le_bytes()); // ET_EXEC
        elf.extend(&0x3Eu16.to_le_bytes()); // EM_X86_64
        elf.extend(&1u32.to_le_bytes());
        // e_entry
        let entry: u64 = 0x0010_0000;
        elf.extend(&entry.to_le_bytes());
        // e_phoff
        elf.extend(&64u64.to_le_bytes());
        // e_shoff
        elf.extend(&0u64.to_le_bytes());
        // e_flags
        elf.extend(&0u32.to_le_bytes());
        // e_ehsize, e_phentsize, e_phnum
        elf.extend(&64u16.to_le_bytes());
        elf.extend(&56u16.to_le_bytes());
        elf.extend(&1u16.to_le_bytes());
        // e_shentsize, e_shnum, e_shstrndx
        elf.extend(&0u16.to_le_bytes());
        elf.extend(&0u16.to_le_bytes());
        elf.extend(&0u16.to_le_bytes());

        // program header (56 bytes)
        // p_type, p_flags
        elf.extend(&1u32.to_le_bytes()); // PT_LOAD
        elf.extend(&5u32.to_le_bytes()); // PF_R | PF_X
        // p_offset
        elf.extend(&0x200u64.to_le_bytes());
        // p_vaddr, p_paddr
        elf.extend(&entry.to_le_bytes());
        elf.extend(&entry.to_le_bytes());
        // p_filesz, p_memsz
        let code: [u8; 3] = [0xf4, 0xeb, 0xfe]; // hlt; jmp -2
        elf.extend(&(code.len() as u64).to_le_bytes());
        elf.extend(&0x1000u64.to_le_bytes());
        // p_align
        elf.extend(&0x1000u64.to_le_bytes());

        // pad to offset 0x200
        if elf.len() < 0x200 {
            elf.resize(0x200, 0);
        }
        // append code
        elf.extend(&code);

        std::fs::write(&hello_path, &elf).expect("failed to write hello.bin");
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let image_path = out_dir.join("initfs.ext2");

    emit_rerun_if_changed(&initfs_dir);

    let status = Command::new("mke2fs")
        .args(["-t", "ext2", "-b", "4096", "-m", "0", "-L", "initfs", "-d"])
        .arg(&initfs_dir)
        .arg(&image_path)
        .arg("4096")
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            panic!("mke2fs failed while generating initfs.ext2");
        }
        Err(e) => {
            panic!("failed to execute mke2fs: {e}. Please install e2fsprogs (mke2fs).");
        }
    }
}

fn emit_rerun_if_changed(path: &Path) {
    if let Ok(metadata) = fs::metadata(path) {
        if metadata.is_file() {
            println!("cargo:rerun-if-changed={}", path.display());
        } else if metadata.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    emit_rerun_if_changed(&entry.path());
                }
            }
        }
    }
}
