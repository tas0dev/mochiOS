use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[allow(unused)]
fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let image_path = out_dir.join("initfs.ext2");
    let stage_dir = out_dir.join("initfs_stage");

    if stage_dir.exists() {
        let _ = fs::remove_dir_all(&stage_dir);
    }
    fs::create_dir_all(&stage_dir).expect("failed to create initfs stage dir");

    let builder_script = manifest_dir.join("scripts/build-user-elf.sh");
    if builder_script.exists() {
        match Command::new("sh").arg(&builder_script).current_dir(&manifest_dir).output() {
            Ok(out) => {
                if !out.status.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    println!("cargo:warning=build-user-elf.sh failed: exit={} stderr=\n{}", out.status, stderr);
                } else {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    println!("cargo:warning=build-user-elf.sh output:\n{}", stdout);
                }
            }
            Err(e) => println!("cargo:warning=failed to run build-user-elf.sh: {}", e),
        }
    }

    let initfs_src = manifest_dir.join("src/initfs");
    if initfs_src.exists() {
        fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
            std::fs::create_dir_all(dst)?;
            for entry in std::fs::read_dir(src)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                let src_path = entry.path();
                let dst_path = dst.join(entry.file_name());
                if file_type.is_dir() {
                    copy_dir_recursive(&src_path, &dst_path)?;
                } else if file_type.is_file() {
                    std::fs::copy(&src_path, &dst_path)?;
                }
            }
            Ok(())
        }

        if let Err(e) = copy_dir_recursive(&initfs_src, &stage_dir) {
            panic!("failed to copy initfs files: {}", e);
        }
    }

    // emit_rerun_if_changed(&manifest_dir.join("src/services/shell"));
    // emit_rerun_if_changed(&manifest_dir.join("src/services/keyboard"));

    // copy_service(&manifest_dir, "shell", &stage_dir);
    // copy_service(&manifest_dir, "keyboard", &stage_dir);

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
/*
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

fn find_service_bin(manifest_dir: &Path, name: &str) -> Option<PathBuf> {
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let profile_dir = if profile == "release" { "release" } else { "debug" };

    if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        let candidate = target_dir
            .join("x86_64-unknown-none")
            .join(profile_dir)
            .join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let candidates = [
        manifest_dir
            .join("target/x86_64-unknown-none")
            .join(profile_dir)
            .join(name),
        manifest_dir
            .join("src/services")
            .join(name)
            .join("target/x86_64-unknown-none")
            .join(profile_dir)
            .join(name),
    ];

    candidates.into_iter().find(|p| p.is_file())
}

fn build_service(manifest_dir: &Path, name: &str) -> Option<PathBuf> {
    if env::var("SWIFTCORE_SKIP_SHELL_BUILD").ok().as_deref() == Some("1") {
        return None;
    }

    let svc_dir = manifest_dir.join("src/services").join(name);
    if !svc_dir.is_dir() {
        return None;
    }

    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&svc_dir)
        .env("SWIFTCORE_SKIP_SHELL_BUILD", "1")
        .args(["build", "--target", "x86_64-unknown-none"]);

    if profile == "release" {
        cmd.arg("--release");
    }

    let status = cmd
        .status();

    match status {
        Ok(s) if s.success() => find_service_bin(manifest_dir, name),
        Ok(_) => None,
        Err(_) => None,
    }
}

fn copy_service(manifest_dir: &Path, name: &str, stage_dir: &Path) {
    let env_key = match name {
        "shell" => "SWIFTCORE_SHELL_BIN",
        "keyboard" => "SWIFTCORE_KEYBOARD_BIN",
        _ => "SWIFTCORE_SERVICE_BIN",
    };

    let env_bin = env::var(env_key).ok().and_then(|p| {
        let path = PathBuf::from(p);
        if path.is_file() { Some(path) } else { None }
    });

    let bin = env_bin
        .or_else(|| find_service_bin(manifest_dir, name))
        .or_else(|| build_service(manifest_dir, name));

    if let Some(bin) = bin {
        let dest = stage_dir.join(format!("{}.service", name));
        let _ = fs::copy(&bin, &dest);
    } else {
        println!(
            "cargo:warning=initfs: {} binary not found; set {} or build service crate",
            name, env_key
        );
    }
}
*/