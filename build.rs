use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let image_path = out_dir.join("initfs.ext2");
    let stage_dir = out_dir.join("initfs_stage");

    if stage_dir.exists() {
        let _ = fs::remove_dir_all(&stage_dir);
    }
    fs::create_dir_all(&stage_dir).expect("failed to create initfs stage dir");

    emit_rerun_if_changed(&manifest_dir.join("src/apps/shell"));

    let shell_bin = find_shell_bin(&manifest_dir)
        .or_else(|| build_shell(&manifest_dir));

    if let Some(shell_bin) = shell_bin {
        let dest = stage_dir.join("shell");
        fs::copy(&shell_bin, &dest).expect("failed to copy shell binary into initfs");
    } else {
        println!("cargo:warning=initfs: shell binary not found; set SWIFTCORE_SHELL_BIN or build shell crate");
    }

    let status = Command::new("mke2fs")
        .args([
            "-t",
            "ext2",
            "-b",
            "4096",
            "-m",
            "0",
            "-L",
            "initfs",
            "-d",
        ])
        .arg(&stage_dir)
        .arg(&image_path)
        .arg("4096")
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            panic!("mke2fs failed while generating initfs.ext2");
        }
        Err(e) => {
            panic!(
                "failed to execute mke2fs: {e}. Please install e2fsprogs (mke2fs)."
            );
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

fn find_shell_bin(manifest_dir: &Path) -> Option<PathBuf> {
    if let Ok(path) = env::var("SWIFTCORE_SHELL_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    let candidates = [
        manifest_dir.join("target/x86_64-unknown-none/debug/shell"),
        manifest_dir.join("target/x86_64-unknown-none/release/shell"),
        manifest_dir.join("src/apps/shell/target/x86_64-unknown-none/debug/shell"),
        manifest_dir.join("src/apps/shell/target/x86_64-unknown-none/release/shell"),
    ];

    candidates.into_iter().find(|p| p.is_file())
}

fn build_shell(manifest_dir: &Path) -> Option<PathBuf> {
    if env::var("SWIFTCORE_SKIP_SHELL_BUILD").ok().as_deref() == Some("1") {
        return None;
    }

    let shell_dir = manifest_dir.join("src/apps/shell");
    if !shell_dir.is_dir() {
        return None;
    }

    let status = Command::new("cargo")
        .current_dir(&shell_dir)
        .env("SWIFTCORE_SKIP_SHELL_BUILD", "1")
        .args(["build", "--target", "x86_64-unknown-none"]) 
        .status();

    match status {
        Ok(s) if s.success() => find_shell_bin(manifest_dir),
        Ok(_) => None,
        Err(_) => None,
    }
}
