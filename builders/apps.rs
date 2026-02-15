use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::utils::{emit_rerun_if_changed, find_binary_in_dir, find_target_spec};

/// アプリケーションをビルドして指定ディレクトリにコピー
pub fn build_apps(apps_dir: &Path, output_dir: &Path, extension: &str) {
    println!("cargo:rerun-if-changed={}", apps_dir.display());

    let entries = match fs::read_dir(apps_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let cargo_toml = path.join("Cargo.toml");
        if !cargo_toml.exists() {
            continue;
        }

        let app_name = path.file_name().unwrap().to_string_lossy();
        println!("Building app: {}", app_name);

        // アプリのソースファイルを明示的に監視
        println!("cargo:rerun-if-changed={}", cargo_toml.display());
        let src_dir = path.join("src");
        if src_dir.is_dir() {
            emit_rerun_if_changed(&src_dir);
        }

        // カスタムターゲットファイルを探す
        let target_spec = find_target_spec(&path);

        // cargoでアプリをビルド
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--release"]);

        // カスタムターゲットが見つかった場合は指定
        if let Some(target) = &target_spec {
            cmd.arg("--target").arg(target);
            println!("  Using target: {}", target);
        } else {
            // デフォルトは ELF (for newlib)
            let default_target = "x86_64-unknown-none";
            cmd.arg("--target").arg(default_target);
            println!("  Using default target: {}", default_target);
        }

        let output = cmd.current_dir(&path).output();

        match output {
            Ok(output) => {
                if output.status.success() {
                    // ビルド成果物を探す
                    let target_dir = path.join("target");
                    let target_name = if let Some(p) = &target_spec {
                        Path::new(p)
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                    } else {
                        Some("x86_64-unknown-none".to_string())
                    };

                    if let Some(elf_path) = find_built_binary(&target_dir, target_name.as_deref())
                    {
                        let dest_name = format!("{}.{}", app_name, extension);
                        let dest = output_dir.join(&dest_name);
                        if let Err(e) = fs::copy(&elf_path, &dest) {
                            println!(
                                "cargo:warning=Failed to copy {} to output: {}",
                                dest_name, e
                            );
                        } else {
                            println!(
                                "Copied {} to {} (from {})",
                                dest_name,
                                output_dir.display(),
                                elf_path.display()
                            );
                        }
                    } else {
                        println!("cargo:warning=Built binary not found for {}", app_name);
                    }
                } else {
                    println!("cargo:warning=Failed to build app: {}", app_name);
                    // エラー出力を表示
                    if !output.stderr.is_empty() {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        for line in stderr.lines().take(10) {
                            println!("cargo:warning=  {}", line);
                        }
                    }
                }
            }
            Err(e) => {
                println!(
                    "cargo:warning=Failed to execute cargo for {}: {}",
                    app_name, e
                );
            }
        }
    }
}

fn find_built_binary(target_dir: &Path, target_name: Option<&str>) -> Option<PathBuf> {
    // カスタムターゲットが指定されている場合はそのディレクトリを優先
    if let Some(target) = target_name {
        let custom_target = target_dir.join(format!("{}/release", target));
        if custom_target.is_dir() {
            if let Some(binary) = find_binary_in_dir(&custom_target) {
                return Some(binary);
            }
        }
    }

    // x86_64-swiftcore/release/ を優先的に探す
    let custom_target = target_dir.join("x86_64-swiftcore/release");
    if custom_target.is_dir() {
        if let Some(binary) = find_binary_in_dir(&custom_target) {
            return Some(binary);
        }
    }

    // 通常のrelease/を探す
    let release_dir = target_dir.join("release");
    if release_dir.is_dir() {
        if let Some(binary) = find_binary_in_dir(&release_dir) {
            return Some(binary);
        }
    }

    None
}
