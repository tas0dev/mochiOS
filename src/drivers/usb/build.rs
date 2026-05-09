use std::env;
use std::path::{Path, PathBuf};

fn cargo_toml_has_workspace(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|s| {
            s.lines()
                .map(|line| line.trim())
                .any(|line| line == "[workspace]")
        })
        .unwrap_or(false)
}

fn find_project_root(manifest_dir: &Path) -> PathBuf {
    if let Ok(workspace_dir) = env::var("CARGO_WORKSPACE_DIR") {
        return PathBuf::from(workspace_dir);
    }

    for ancestor in manifest_dir.ancestors().skip(1) {
        if ancestor.join("ramfs").join("lib").exists() {
            return ancestor.to_path_buf();
        }
    }

    for ancestor in manifest_dir.ancestors().skip(1) {
        let cargo_toml = ancestor.join("Cargo.toml");
        if cargo_toml.exists() && cargo_toml_has_workspace(&cargo_toml) {
            return ancestor.to_path_buf();
        }
    }

    manifest_dir.to_path_buf()
}

fn main() {
    // Skip when building host PoC
    if std::env::var("MOCHI_HOST_POC").is_ok() {
        println!("cargo:warning=MOCHI_HOST_POC set; skipping mochiOS linker flags in drivers/usb/build.rs");
        return;
    }

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let manifest_path = Path::new(&manifest_dir);
    let project_root = find_project_root(manifest_path);

    let libs_dir = project_root.join("ramfs").join("lib");

    // ライブラリ検索パスを追加
    println!("cargo:rustc-link-search=native={}", libs_dir.display());

    // crt0.o をリンク
    println!("cargo:rustc-link-arg={}/crt0.o", libs_dir.display());

    // 静的リンクを指定し、PIEを無効化する
    println!("cargo:rustc-link-arg=-static");
    println!("cargo:rustc-link-arg=-no-pie");

    // カスタムリンカースクリプトを使用してロードアドレスを0x800000に設定
    println!("cargo:rustc-link-arg=-T{}/linker.ld", manifest_dir);
    println!("cargo:rerun-if-changed={}", manifest_path.join("linker.ld").display());

    // 重複シンボルを許可（最初に見つかったものを使用）
    println!("cargo:rustc-link-arg=--allow-multiple-definition");

    // ライブラリをリンク
    println!("cargo:rustc-link-lib=static=c"); // libc.a
    println!("cargo:rustc-link-lib=static=g"); // libg.a
    println!("cargo:rustc-link-lib=static=m"); // libm.a

    // std の unwind クレートが libgcc_s を要求するため libg.a を libgcc_s.a として提供
    let libgcc_s = libs_dir.join("libgcc_s.a");
    let libg = libs_dir.join("libg.a");
    if !libgcc_s.exists() && libg.exists() {
        let tmp_name = format!(
            "libgcc_s.a.tmp.{}.{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("usb-build")
        );
        let libgcc_tmp = libs_dir.join(tmp_name);
        if let Err(err) = std::fs::copy(&libg, &libgcc_tmp) {
            panic!(
                "failed to copy {} to {} for static gcc_s linking: {}",
                libg.display(),
                libgcc_tmp.display(),
                err
            );
        }
        if let Err(err) = std::fs::rename(&libgcc_tmp, &libgcc_s) {
            if libgcc_s.exists() {
                let _ = std::fs::remove_file(&libgcc_tmp);
            } else {
                let _ = std::fs::remove_file(&libgcc_tmp);
                panic!(
                    "failed to rename {} to {} for static gcc_s linking: {}",
                    libgcc_tmp.display(),
                    libgcc_s.display(),
                    err
                );
            }
        }
    }
    println!("cargo:rustc-link-lib=static=gcc_s");

    println!("cargo:rerun-if-changed={}", libs_dir.join("libc.a").display());
    println!("cargo:rerun-if-changed={}", libs_dir.join("libg.a").display());
    println!("cargo:rerun-if-changed={}", libs_dir.join("libm.a").display());
}

