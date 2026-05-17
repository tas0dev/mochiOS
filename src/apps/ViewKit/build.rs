use std::env;
use std::path::{Path, PathBuf};


#[derive(Debug)]
#[allow(dead_code)]
struct ComponentBuild {
    name: String,
}

fn find_project_root(manifest_dir: &Path) -> PathBuf {
    if let Ok(workspace_dir) = env::var("CARGO_WORKSPACE_DIR") {
        return PathBuf::from(workspace_dir);
    }
    for ancestor in manifest_dir.ancestors() {
        if ancestor.join("ramfs").join("lib").exists()
            || ancestor.join("ramfs").join("Libraries").exists()
        {
            return ancestor.to_path_buf();
        }
    }
    manifest_dir.to_path_buf()
}

fn find_libs_dir(project_root: &Path) -> PathBuf {
    let candidates = [
        project_root.join("ramfs").join("lib"),
        project_root.join("ramfs").join("Libraries"),
    ];
    for candidate in candidates {
        if candidate.join("libc.a").exists() {
            return candidate;
        }
    }
    project_root.join("ramfs").join("lib")
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let manifest_path = Path::new(&manifest_dir);
    let target = env::var("TARGET").unwrap_or_default();

    println!("cargo:rerun-if-env-changed=MOCHI_HOST_POC");
    println!("cargo:rerun-if-env-changed=TARGET");

    if env::var("MOCHI_HOST_POC").is_ok() || target.contains("unknown-linux-gnu") {
        return;
    }

    let project_root = find_project_root(manifest_path);
    let libs_dir = find_libs_dir(&project_root);

    println!("cargo:rustc-link-search=native={}", libs_dir.display());
    println!("cargo:rustc-link-arg={}/crt0.o", libs_dir.display());
    println!("cargo:rustc-link-arg=-static");
    println!("cargo:rustc-link-arg=-no-pie");
    println!("cargo:rustc-link-arg=-T{}/linker.ld", manifest_dir);
    println!("cargo:rustc-link-arg=--allow-multiple-definition");
    println!("cargo:rustc-link-lib=static=c");
    println!("cargo:rustc-link-lib=static=g");
    println!("cargo:rustc-link-lib=static=m");
    println!("cargo:rustc-link-lib=static=nosys");
    // Ensure libunwind is linked so symbols like _Unwind_GetIP are resolved
    println!("cargo:rustc-link-lib=static=unwind");
    // Also pass libunwind.a directly to the linker to ensure symbols are present
    println!("cargo:rustc-link-arg={}/libunwind.a", libs_dir.display());
    // Link libextra.a which provides minimal getcwd implementation used by libstd
    println!("cargo:rustc-link-arg={}/libextra.a", libs_dir.display());

    let libgcc_s = libs_dir.join("libgcc_s.a");
    let libg = libs_dir.join("libg.a");
    if !libgcc_s.exists() && libg.exists() {
        let tmp = libs_dir.join("libgcc_s.a.tmp");
        if let Err(err) = std::fs::copy(&libg, &tmp) {
            panic!(
                "failed to copy {} to {} for static gcc_s linking: {}",
                libg.display(),
                tmp.display(),
                err
            );
        }
        if let Err(err) = std::fs::rename(&tmp, &libgcc_s) {
            let _ = std::fs::remove_file(&tmp);
            if !libgcc_s.exists() {
                panic!(
                    "failed to rename {} to {} for static gcc_s linking: {}",
                    tmp.display(),
                    libgcc_s.display(),
                    err
                );
            }
        }
    }
    println!("cargo:rustc-link-lib=static=gcc_s");
    println!(
        "cargo:rerun-if-changed={}",
        manifest_path.join("linker.ld").display()
    );
    println!("cargo:rerun-if-changed={}", libs_dir.join("libc.a").display());
}
