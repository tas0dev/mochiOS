use std::env;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");

    let root_dir = Path::new(&manifest_dir)
        .ancestors()
        .nth(3)
        .expect("failed to determine project root");

    // リンカスクリプトを指定
    let linker_script = Path::new(&manifest_dir).join("linker.ld");
    if linker_script.exists() {
        println!("cargo:rustc-link-arg=-T{}", linker_script.display());
        println!("cargo:rerun-if-changed={}", linker_script.display());
    }

    let libs_dir = root_dir.join("ramfs").join("Libraries");

    if libs_dir.exists() {
        println!("cargo:rustc-link-search=native={}", libs_dir.display());
        println!("cargo:rustc-link-arg={}/crt0.o", libs_dir.display());
        println!("cargo:rustc-link-arg=-static");
        println!("cargo:rustc-link-arg=-no-pie");
        println!("cargo:rustc-link-arg=--allow-multiple-definition");

        println!("cargo:rustc-link-lib=static=c");
        println!("cargo:rustc-link-lib=static=g");
        let libgcc_s = libs_dir.join("libgcc_s.a");
        let libg = libs_dir.join("libg.a");
        if !libgcc_s.exists() && libg.exists() {
            let _ = std::fs::copy(&libg, &libgcc_s);
        }
        println!("cargo:rustc-link-lib=static=gcc_s");
        println!("cargo:rustc-link-lib=static=m");
        println!("cargo:rustc-link-lib=static=nosys");
    }

    println!("cargo:rerun-if-changed=../../../ramfs/Libraries/libc.a");
}

