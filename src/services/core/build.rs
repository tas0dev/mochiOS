use std::env;
use std::path::Path;

fn main() {
    let target = env::var("TARGET").unwrap_or_else(|_| "x86_64-unknown-none".to_string());
    
    // newlib ライブラリへのパスを設定
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    
    // ルートのtargetディレクトリを使用
    let root_dir = Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();

    // リンカスクリプトを指定 (target JSON から削除したため build.rs で指定)
    let linker_script = Path::new(&manifest_dir).join("linker.ld");
    if linker_script.exists() {
        println!("cargo:rustc-link-arg=-T{}", linker_script.display());
        println!("cargo:rerun-if-changed={}", linker_script.display());
    }
    
    let newlib_dir = root_dir
        .join("target")
        .join(&target)
        .join(&profile)
        .join("newlib_install")
        .join("x86_64-elf")
        .join("lib");
    
    // ramfsディレクトリ（crt0.oとライブラリが配置される場所）
    let ramfs_dir = root_dir.join("ramfs");
    
    if ramfs_dir.exists() {
        // ライブラリ検索パスを追加
        println!("cargo:rustc-link-search=native={}", ramfs_dir.display());
        
        // crt0.o をリンク
        println!("cargo:rustc-link-arg={}/crt0.o", ramfs_dir.display());
        
        // 静的リンクを指定し、PIEを無効化する
        println!("cargo:rustc-link-arg=-static");
        println!("cargo:rustc-link-arg=-no-pie");
        
        // 重複シンボルを許可
        println!("cargo:rustc-link-arg=--allow-multiple-definition");
        
        // ライブラリをリンク
        println!("cargo:rustc-link-lib=static=c");
        println!("cargo:rustc-link-lib=static=g");
        println!("cargo:rustc-link-lib=static=m");
        println!("cargo:rustc-link-lib=static=nosys");
    } else if newlib_dir.exists() {
        // フォールバック（ramfsがまだ存在しない場合）
        println!("cargo:rustc-link-search=native={}", newlib_dir.display());
        println!("cargo:rustc-link-lib=static=c");
        println!("cargo:rustc-link-lib=static=m");
        println!("cargo:rustc-link-lib=static=nosys");
        
        println!("cargo:rustc-link-arg=-static");
        println!("cargo:rustc-link-arg=-no-pie");
        println!("cargo:rustc-link-arg=--allow-multiple-definition");
    }
}
