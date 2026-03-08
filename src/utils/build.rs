use std::env;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let project_root = Path::new(&manifest_dir)
        .ancestors()
        .nth(2)
        .expect("failed to determine project root");

    let fs_dir = project_root.join("fs");

    println!("cargo:rustc-link-search=native={}", fs_dir.display());
    println!("cargo:rustc-link-arg={}/crt0.o", fs_dir.display());
    println!("cargo:rustc-link-arg=-static");
    println!("cargo:rustc-link-arg=-no-pie");
    println!("cargo:rustc-link-lib=static=c");
    println!("cargo:rustc-link-lib=static=g");
    println!("cargo:rustc-link-lib=static=m");

    let libgcc_s = fs_dir.join("libgcc_s.a");
    let libg = fs_dir.join("libg.a");
    if !libgcc_s.exists() && libg.exists() {
        let _ = std::fs::copy(&libg, &libgcc_s);
    }
    println!("cargo:rustc-link-lib=static=gcc_s");

    println!("cargo:rustc-link-arg=-Tlinker.ld");
    println!("cargo:rustc-link-arg=--allow-multiple-definition");

    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=../../fs/libc.a");
}
