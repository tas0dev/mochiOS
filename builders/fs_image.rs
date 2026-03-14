use std::fs;
use std::path::Path;
use std::process::Command;

use super::utils::emit_rerun_if_changed;

/// ディレクトリ以下のファイルの合計バイト数を再帰的に計算する
fn compute_content_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                total += compute_content_size(&path);
            } else if let Ok(meta) = path.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// コンテンツサイズからext2イメージのブロック数を計算する
/// オーバーヘッド 25% + ext2メタデータ用 10MB を加算し、最小 32MB を保証する
fn blocks_for_dir(dir: &Path, block_size: u64) -> u64 {
    let content = compute_content_size(dir);
    let needed = ((content * 5 / 4) + 10 * 1024 * 1024).max(32 * 1024 * 1024);
    (needed + block_size - 1) / block_size
}

/// InitFS (ramfs) 用のext2イメージを生成
pub fn create_initfs_image(ramfs_dir: &Path, output_path: &Path) -> Result<(), String> {
    println!("Creating initfs ext2 image from {}", ramfs_dir.display());

    emit_rerun_if_changed(ramfs_dir);

    let num_blocks = blocks_for_dir(ramfs_dir, 4096);
    println!("initfs: {} 4K-blocks ({} MB)", num_blocks, num_blocks * 4 / 1024);

    let status = Command::new("mke2fs")
        .args(["-t", "ext2", "-b", "4096", "-m", "0", "-L", "initfs", "-d"])
        .arg(ramfs_dir)
        .arg(output_path)
        .arg(num_blocks.to_string())
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

    let num_blocks = blocks_for_dir(fs_dir, 4096);
    println!("rootfs: {} 4K-blocks ({} MB)", num_blocks, num_blocks * 4 / 1024);

    let status = Command::new("mke2fs")
        .args(["-t", "ext2", "-b", "4096", "-m", "0", "-L", "rootfs", "-d"])
        .arg(fs_dir)
        .arg(output_path)
        .arg(num_blocks.to_string())
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
        "Libraries",        // ライブラリ（libc.aなど）を配置
        "Mount",            // マウントしたやつ配置
        "Boot",             // ブートローダー関連のファイルを配置
        "Resources",        // アイコンやUIリソースを配置（ユーザーアプリのリソースはここに置く）
        "Services",         // サービスを配置
        "Logs",             // ログを配置
        "Home",             // ユーザーディレクトリを配置
        "Device",           // デバイスファイル（nullやttyなど）を配置
        "Config",           // 設定ファイルを配置
        "Variables",        // 環境変数や一時ファイルを配置
        "Temp",             // 一時ファイルを配置
    ];
    
    for dir in &dirs {
        let path = fs_dir.join(dir);
        fs::create_dir_all(&path)
            .map_err(|e| format!("Failed to create {}: {}", path.display(), e))?;
        println!("Created directory: {}", path.display());
    }

    // src/resources/ の各サブディレクトリを対応する fs/ ディレクトリにコピー
    // 例: src/resources/System/ → fs/System/
    //     src/resources/Config/ → fs/Config/
    if resources_src.is_dir() {
        for entry in fs::read_dir(resources_src)
            .map_err(|e| format!("Failed to read resources dir: {}", e))?
        {
            let entry = entry.map_err(|e| format!("Failed to read resources entry: {}", e))?;
            let src_path = entry.path();
            if !src_path.is_dir() {
                continue;
            }
            let dir_name = entry.file_name();
            let dst_path = fs_dir.join(&dir_name);
            copy_dir_recursive(&src_path, &dst_path)?;
            println!(
                "Copied resources/{} -> {}",
                dir_name.to_string_lossy(),
                dst_path.display()
            );
        }
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
    fs::create_dir_all(dest_dir)
        .map_err(|e| format!("Failed to create {}: {}", dest_dir.display(), e))?;

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
