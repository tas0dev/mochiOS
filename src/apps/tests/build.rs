use std::env;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let project_root = Path::new(&manifest_dir)
        .ancestors()
        .nth(3)
        .expect("failed to determine project root");

    // 生成されたnewlibとcrt0の場所
    let fs_dir = project_root.join("fs");

    // ライブラリ検索パスを追加
    println!("cargo:rustc-link-search=native={}", fs_dir.display());

    // crt0.o をリンク（Rustにオブジェクトファイルを直接リンクさせるのは難しい場合があるが、
    // ここでは rustc-link-arg でオブジェクトファイルを指定する）
    println!("cargo:rustc-link-arg={}/crt0.o", fs_dir.display());

    // 静的リンクを指定し、PIEを無効化する
    // x86_64-unknown-none はデフォルトでPIEを生成する可能性があるが、
    // newlibはPICなしでビルドされているため、静的リンクを強制する。
    println!("cargo:rustc-link-arg=-static");
    println!("cargo:rustc-link-arg=-no-pie");

    // ライブラリをリンク
    // グループ化して循環参照を解決するのが一般的だが、Rustのリンカ指定だと順序が大事
    println!("cargo:rustc-link-lib=static=c"); // libc.a (userglue入り)
    println!("cargo:rustc-link-lib=static=g"); // libg.a
    println!("cargo:rustc-link-lib=static=m"); // libm.a

    // std の unwind クレートが libgcc_s を要求するため libg.a を libgcc_s.a として提供
    let libgcc_s = fs_dir.join("libgcc_s.a");
    let libg = fs_dir.join("libg.a");
    if !libgcc_s.exists() && libg.exists() {
        let _ = std::fs::copy(&libg, &libgcc_s);
    }
    println!("cargo:rustc-link-lib=static=gcc_s");

    // リンカスクリプトの指定
    println!("cargo:rustc-link-arg=-Tlinker.ld");
    println!("cargo:rustc-link-arg=--allow-multiple-definition");

    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=../../fs/libc.a");
}

