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

    // crt0.o をリンク
    println!("cargo:rustc-link-arg={}/crt0.o", fs_dir.display());

    // 静的リンクを指定し、PIEを無効化する
    println!("cargo:rustc-link-arg=-static");
    println!("cargo:rustc-link-arg=-no-pie");

    // カスタムリンカースクリプトを使用してロードアドレスを0x800000に設定
    println!("cargo:rustc-link-arg=-T{}/linker.ld", manifest_dir);
    println!("cargo:rerun-if-changed=linker.ld");
    
    // 重複シンボルを許可（最初に見つかったものを使用）
    println!("cargo:rustc-link-arg=--allow-multiple-definition");

    // ライブラリをリンク
    println!("cargo:rustc-link-lib=static=c"); // libc.a
    println!("cargo:rustc-link-lib=static=g"); // libg.a
    println!("cargo:rustc-link-lib=static=m"); // libm.a

    println!("cargo:rerun-if-changed=../../fs/libc.a");
}

