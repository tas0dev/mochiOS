use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::utils::{emit_rerun_if_changed, find_binary_in_dir};

#[derive(Debug, Clone)]
pub struct ModuleEntry {
    pub name: &'static str,
    pub dir: &'static str,
    pub version: u16,
    pub deps: &'static [&'static str],
}

pub fn default_modules() -> Vec<ModuleEntry> {
    vec![
        ModuleEntry {
            name: "disk",
            dir: "disk",
            version: 1,
            deps: &[],
        },
        ModuleEntry {
            name: "fs",
            dir: "fs",
            version: 1,
            deps: &["disk"],
        },
    ]
}

pub fn build_module(
    module: &ModuleEntry,
    modules_base_dir: &Path,
    output_dir: &Path,
) -> Result<(), String> {
    let module_dir = modules_base_dir.join(module.dir);
    if !module_dir.exists() {
        return Err(format!(
            "Module directory not found: {}",
            module_dir.display()
        ));
    }

    let cargo_toml = module_dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Err(format!("Cargo.toml not found for module {}", module.name));
    }

    println!("Building module: {}", module.name);
    println!("cargo:rerun-if-changed={}", cargo_toml.display());
    let src_dir = module_dir.join("src");
    if src_dir.is_dir() {
        emit_rerun_if_changed(&src_dir);
    }

    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release"]);
    for key in &[
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_MAKEFLAGS",
        "__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS",
        "CARGO_BUILD_RUSTC",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ] {
        cmd.env_remove(key);
    }

    let output = cmd
        .current_dir(&module_dir)
        .output()
        .map_err(|e| format!("Failed to run cargo for module {}: {}", module.name, e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "Failed to build module {}: status={} STDERR={} STDOUT={}",
            module.name, output.status, stderr, stdout
        ));
    }

    let target_dir = module_dir.join("target");
    let binary_path = find_module_binary(&target_dir).ok_or_else(|| {
        format!(
            "Built binary not found for module {} under {}",
            module.name,
            target_dir.display()
        )
    })?;

    let modules_out_dir = output_dir.join("Modules");
    fs::create_dir_all(&modules_out_dir)
        .map_err(|e| format!("Failed to create {}: {}", modules_out_dir.display(), e))?;
    let cext_path = modules_out_dir.join(format!("{}.cext", module.name));
    build_cext(module, &binary_path, &cext_path)?;

    println!("Generated {}", cext_path.display());
    Ok(())
}

fn find_module_binary(target_dir: &Path) -> Option<PathBuf> {
    for profile in &["release", "debug"] {
        let dir = target_dir.join(format!("x86_64-mochios/{}", profile));
        if dir.is_dir() {
            if let Some(binary) = find_binary_in_dir(&dir) {
                return Some(binary);
            }
        }
        let dir = target_dir.join(profile);
        if dir.is_dir() {
            if let Some(binary) = find_binary_in_dir(&dir) {
                return Some(binary);
            }
        }
    }
    None
}

fn build_cext(module: &ModuleEntry, elf_path: &Path, cext_path: &Path) -> Result<(), String> {
    const MAGIC: [u8; 4] = *b"MCEX";
    const ABI_VERSION: u16 = 1;
    const FIXED_HEADER_SIZE: u32 = 32;

    let elf =
        fs::read(elf_path).map_err(|e| format!("Failed to read {}: {}", elf_path.display(), e))?;
    let name = module.name.as_bytes();
    if name.len() > u16::MAX as usize {
        return Err(format!("Module name too long: {}", module.name));
    }

    let mut metadata = Vec::new();
    metadata.extend_from_slice(name);
    for dep in module.deps {
        let dep_bytes = dep.as_bytes();
        if dep_bytes.len() > u16::MAX as usize {
            return Err(format!("Dependency name too long: {}", dep));
        }
        metadata.extend_from_slice(&(dep_bytes.len() as u16).to_le_bytes());
        metadata.extend_from_slice(dep_bytes);
    }

    let header_size = FIXED_HEADER_SIZE
        .checked_add(u32::try_from(metadata.len()).map_err(|_| "metadata too large".to_string())?)
        .ok_or_else(|| "header size overflow".to_string())?;

    let mut out = Vec::with_capacity(header_size as usize + elf.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&ABI_VERSION.to_le_bytes());
    out.extend_from_slice(&module.version.to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&(module.deps.len() as u16).to_le_bytes());
    out.extend_from_slice(&header_size.to_le_bytes());
    out.extend_from_slice(&(elf.len() as u64).to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&metadata);
    out.extend_from_slice(&elf);

    fs::write(cext_path, out)
        .map_err(|e| format!("Failed to write {}: {}", cext_path.display(), e))?;
    Ok(())
}
