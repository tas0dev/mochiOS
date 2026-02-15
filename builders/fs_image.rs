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
        .arg("16384") // 64MB (16384 * 4KB blocks) - initfs用に十分なサイズ
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

/// newlibライブラリをディレクトリにコピー
pub fn copy_newlib_libs(libc_dir: &Path, dest_dir: &Path) -> Result<(), String> {
    // crt0.oをコピー
    let crt0_src = libc_dir.join("crt0.o");
    let crt0_dest = dest_dir.join("crt0.o");
    fs::copy(&crt0_src, &crt0_dest).map_err(|e| {
        format!(
            "Failed to copy crt0.o to {}: {}",
            dest_dir.display(),
            e
        )
    })?;
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
        println!("Copied {} to {} (from {})", lib, dest_dir.display(), src.display());
    }

    Ok(())
}
