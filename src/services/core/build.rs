use std::env;

fn main() {
    let target = env::var("TARGET").unwrap_or_else(|_| "x86_64-unknown-none".to_string());
    
    // newlib ライブラリへのパスを設定
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    
    // ルートのtargetディレクトリを使用
    let root_dir = std::path::Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    
    let newlib_dir = root_dir
        .join("target")
        .join(&target)
        .join(&profile)
        .join("newlib_install")
        .join("x86_64-elf")
        .join("lib");
    
    if newlib_dir.exists() {
        println!("cargo:rustc-link-search=native={}", newlib_dir.display());
        println!("cargo:rustc-link-lib=static=c");
        println!("cargo:rustc-link-lib=static=m");
        println!("cargo:rustc-link-lib=static=nosys");
        
        // 重複シンボルを許可
        println!("cargo:rustc-link-arg=--allow-multiple-definition");
    }
}
