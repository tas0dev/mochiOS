mod builders;

use std::env;
use std::fs;
use std::path::PathBuf;

use builders::{
    build_apps, build_newlib, build_service, build_user_libs, copy_newlib_libs,
    create_ext2_image, create_initfs_image, parse_service_index,
};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // ramfsとfsディレクトリを作成
    let ramfs_dir = manifest_dir.join("ramfs");
    let fs_dir = manifest_dir.join("fs");

    for dir in &[&ramfs_dir, &fs_dir] {
        if !dir.is_dir() {
            fs::create_dir_all(dir).expect(&format!(
                "Failed to create directory: {}",
                dir.display()
            ));
        }
    }

    // newlibのビルド
    let newlib_src_dir = manifest_dir.join("src/lib");
    if !newlib_src_dir.exists() {
        panic!("Newlib source not found at {}", newlib_src_dir.display());
    }
    build_newlib(&newlib_src_dir);

    // newlibのインストールディレクトリを取得
    let target = env::var("TARGET").unwrap_or("x86_64-unknown-uefi".to_string());
    let profile = env::var("PROFILE").unwrap_or("debug".to_string());
    let target_dir = PathBuf::from(env::var("CARGO_TARGET_DIR").unwrap_or("target".to_string()));

    let abs_target_dir = if target_dir.is_absolute() {
        target_dir
    } else {
        manifest_dir.join(target_dir)
    };

    let newlib_install_dir = abs_target_dir
        .join(&target)
        .join(&profile)
        .join("newlib_install");

    let libc_dir = newlib_install_dir.join("x86_64-elf").join("lib");

    // ユーザーライブラリをビルド
    let user_src_dir = manifest_dir.join("src/user");
    build_user_libs(&user_src_dir, &libc_dir);

    // newlibライブラリをramfsとfsにコピー
    copy_newlib_libs(&libc_dir, &ramfs_dir).expect("Failed to copy newlib libs to ramfs");
    copy_newlib_libs(&libc_dir, &fs_dir).expect("Failed to copy newlib libs to fs");

    // services/index.toml を解析
    let index_path = manifest_dir.join("src/services/index.toml");
    println!("cargo:rerun-if-changed={}", index_path.display());

    let services = parse_service_index(&index_path).expect("Failed to parse index.toml");

    // サービスをビルド
    let services_base_dir = manifest_dir.join("src/services");

    for service in &services {
        let output_dir = if service.fs_type == "initfs" {
            &ramfs_dir
        } else {
            &fs_dir
        };

        if let Err(e) = build_service(service, &services_base_dir, output_dir) {
            println!("cargo:warning=Failed to build service {}: {}", service.name, e);
        }
    }

    // アプリケーションをビルド（全てfsへ）
    let apps_dir = manifest_dir.join("src/apps");
    if apps_dir.is_dir() {
        build_apps(&apps_dir, &fs_dir, "elf");
    }

    // initfs イメージを生成
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let initfs_image_path = out_dir.join("initfs.ext2");

    create_initfs_image(&ramfs_dir, &initfs_image_path).expect("Failed to create initfs image");

    // ext2 イメージを生成
    let ext2_image_path = out_dir.join("rootfs.ext2");
    create_ext2_image(&fs_dir, &ext2_image_path).expect("Failed to create ext2 image");

    // make_image.sh を実行（UEFIイメージ作成）
    let mkimage_script = manifest_dir.join("scripts/make_image.sh");
    if mkimage_script.exists() {
        let _ = std::process::Command::new(mkimage_script).status();
    }

    println!("Build completed successfully!");
    println!("  ramfs/ -> {}", initfs_image_path.display());
    println!("  fs/    -> {}", ext2_image_path.display());
}
