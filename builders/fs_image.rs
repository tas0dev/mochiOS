use std::fs;
use std::path::Path;
use std::process::Command;

use super::utils::emit_rerun_if_changed;

/// InitFS (ramfs) 用のext2イメージを生成
pub fn create_initfs_image(ramfs_dir: &Path, output_path: &Path) -> Result<(), String> {
    println!("Creating initfs ext2 image from {}", ramfs_dir.display());

    emit_rerun_if_changed(ramfs_dir);

    let status = Command::new("mke2fs")
        .args(["-t", "ext2", "-b", "4096", "-m", "0", "-L", "initfs", "-d"])
        .arg(ramfs_dir)
        .arg(output_path)
        .arg("32768") // 128MB (32768 * 4KB blocks) - increase size to fit static libs like libgcc_s.a
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("Created initfs image at {}", output_path.display());
            Ok(())
        }
        Ok(_) => Err("mke2fs failed while generating initfs image".to_string()),
        Err(e) => Err(format!(
            "Failed to execute mke2fs: {}. Please install e2fsprogs (mke2fs).",
            e
        )),
    }
}

/// EXT2 ファイルシステムイメージを生成
pub fn create_ext2_image(fs_dir: &Path, output_path: &Path) -> Result<(), String> {
    println!("Creating ext2 filesystem image from {}", fs_dir.display());

    emit_rerun_if_changed(fs_dir);

    let status = Command::new("mke2fs")
        .args(["-t", "ext2", "-b", "4096", "-m", "0", "-L", "rootfs", "-d"])
        .arg(fs_dir)
        .arg(output_path)
        .arg("32768") // 128MB (32768 * 4KB blocks)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("Created ext2 image at {}", output_path.display());
            Ok(())
        }
        Ok(_) => Err("mke2fs failed while generating ext2 image".to_string()),
        Err(e) => Err(format!(
            "Failed to execute mke2fs: {}. Please install e2fsprogs (mke2fs).",
            e
        )),
    }
}

/// fsディレクトリの標準レイアウトを作成
pub fn setup_fs_layout(fs_dir: &Path, resources_src: &Path) -> Result<(), String> {
    let dirs = [
        "System",           // システム（カーネルやカーネルに関連するファイルを配置）
        "Applications",     // ユーザーアプリケーションを配置
        "Binaries",         // コマンドやユーティリティを配置
        "Librarys",         // ライブラリ（libc.aなど）を配置
        "Devices",          // マウントしたデバイスなどを配置
        "Boot",             // ブートローダー関連のファイルを配置
        "Resources",        // アイコンやUIリソースを配置（ユーザーアプリのリソースはここに置く）
        "Services",         // サービスを配置
        "Logs",             // ログを配置
        "Home",             // ユーザーディレクトリを配置
    ];
    
    for dir in &dirs {
        let path = fs_dir.join(dir);
        fs::create_dir_all(&path)
            .map_err(|e| format!("Failed to create {}: {}", path.display(), e))?;
        println!("Created directory: {}", path.display());
    }

    // src/resources/ 以下をすべて fs/System/ にコピー
    if resources_src.is_dir() {
        let system_dir = fs_dir.join("System");
        copy_dir_recursive(resources_src, &system_dir)?;
        println!(
            "Copied resources from {} to {}",
            resources_src.display(),
            system_dir.display()
        );
    }

    Ok(())
}

/// ディレクトリを再帰的にコピーする
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create {}: {}", dst.display(), e))?;

    for entry in fs::read_dir(src)
        .map_err(|e| format!("Failed to read {}: {}", src.display(), e))?
    {
        let entry =
            entry.map_err(|e| format!("Failed to read entry in {}: {}", src.display(), e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(|e| {
                format!(
                    "Failed to copy {} to {}: {}",
                    src_path.display(),
                    dst_path.display(),
                    e
                )
            })?;
            println!("Copied: {} -> {}", src_path.display(), dst_path.display());
        }
    }

    Ok(())
}

/// newlibライブラリをディレクトリにコピー
pub fn copy_newlib_libs(libc_dir: &Path, dest_dir: &Path) -> Result<(), String> {
    // crt0.oをコピー
    let crt0_src = libc_dir.join("crt0.o");
    let crt0_dest = dest_dir.join("crt0.o");
    fs::copy(&crt0_src, &crt0_dest)
        .map_err(|e| format!("Failed to copy crt0.o to {}: {}", dest_dir.display(), e))?;
    println!("Copied crt0.o to {}", dest_dir.display());

    // ライブラリをコピー
    let libs = ["libc.a", "libg.a", "libm.a", "libnosys.a"];
    for lib in &libs {
        let src = libc_dir.join(lib);
        let dest = dest_dir.join(lib);
        fs::copy(&src, &dest).map_err(|e| {
            format!(
                "Failed to copy {} to {}: {}. Make sure newlib is built correctly.",
                lib,
                dest_dir.display(),
                e
            )
        })?;
        println!(
            "Copied {} to {} (from {})",
            lib,
            dest_dir.display(),
            src.display()
        );
    }

    Ok(())
}
