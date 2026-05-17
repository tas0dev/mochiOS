use std::env;
use std::path::{Path, PathBuf};

fn find_project_root(manifest_dir: &Path) -> PathBuf {
    if let Ok(workspace_dir) = env::var("CARGO_WORKSPACE_DIR") {
        return PathBuf::from(workspace_dir);
    }
    for ancestor in manifest_dir.ancestors() {
        if ancestor.join("ramfs").join("lib").exists() {
            return ancestor.to_path_buf();
        }
    }
    manifest_dir.to_path_buf()
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let manifest_path = Path::new(&manifest_dir);
    let project_root = find_project_root(manifest_path);
    let libs_dir = project_root.join("ramfs").join("lib");

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
