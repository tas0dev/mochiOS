use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::utils::{emit_rerun_if_changed, find_binary_in_dir, find_target_spec};

/// サービスインデックスの情報
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ServiceEntry {
    pub name: String,
    pub dir: String,
    pub fs_type: String,
    pub description: String,
    pub autostart: bool,
    pub order: u32,
}

/// index.tomlを解析してサービス情報を取得
pub fn parse_service_index(index_path: &Path) -> Result<Vec<ServiceEntry>, String> {
    let content = fs::read_to_string(index_path)
        .map_err(|e| format!("Failed to read index.toml: {}", e))?;

    // 簡易的なTOML解析（tomlクレートを使わずに）
    let mut services = Vec::new();

    let mut current_service = String::new();
    let mut current_dir = String::new();
    let mut current_fs = String::new();
    let mut current_desc = String::new();
    let mut current_autostart = false;
    let mut current_order = 999;

    for line in content.lines() {
        let line = line.trim();

        // [core.service] または [core.service.NAME] を解析
        if line.starts_with("[core.service") && line.ends_with(']') {
            // 前のサービスを保存
            if !current_service.is_empty() {
                services.push(ServiceEntry {
                    name: current_service.clone(),
                    dir: current_dir.clone(),
                    fs_type: current_fs.clone(),
                    description: current_desc.clone(),
                    autostart: current_autostart,
                    order: current_order,
                });
            }

            // 新しいサービス名を取得
            if line == "[core.service]" {
                current_service = "core".to_string();
            } else if line.starts_with("[core.service.") {
                let start = "[core.service.".len();
                let end = line.len() - 1;
                current_service = line[start..end].to_string();
            }
            
            current_dir.clear();
            current_fs.clear();
            current_desc.clear();
            current_autostart = false;
            current_order = 999;
        } else if line.starts_with("dir = ") {
            current_dir = line["dir = ".len()..]
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
        } else if line.starts_with("fs = ") || line.starts_with("fs_type = ") {
            let prefix = if line.starts_with("fs = ") {
                "fs = "
            } else {
                "fs_type = "
            };
            current_fs = line[prefix.len()..]
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
        } else if line.starts_with("description = ") {
            current_desc = line["description = ".len()..]
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
        } else if line.starts_with("autostart = ") {
            current_autostart = line["autostart = ".len()..]
                .trim()
                .parse()
                .unwrap_or(false);
        } else if line.starts_with("order = ") {
            current_order = line["order = ".len()..]
                .trim()
                .parse()
                .unwrap_or(999);
        }
    }

    // 最後のサービスを保存
    if !current_service.is_empty() {
        services.push(ServiceEntry {
            name: current_service,
            dir: current_dir,
            fs_type: current_fs,
            description: current_desc,
            autostart: current_autostart,
            order: current_order,
        });
    }

    // order順にソート
    services.sort_by_key(|s| s.order);

    Ok(services)
}

/// サービスをビルドして指定ディレクトリにコピー
pub fn build_service(
    service: &ServiceEntry,
    services_base_dir: &Path,
    output_dir: &Path,
) -> Result<(), String> {
    let service_dir = services_base_dir.join(&service.dir);

    if !service_dir.exists() {
        return Err(format!(
            "Service directory not found: {}",
            service_dir.display()
        ));
    }

    let cargo_toml = service_dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Err(format!(
            "Cargo.toml not found for service {}",
            service.name
        ));
    }

    println!("Building service: {} ({})", service.name, service.description);

    // ソースファイルを監視
    println!("cargo:rerun-if-changed={}", cargo_toml.display());
    let src_dir = service_dir.join("src");
    if src_dir.is_dir() {
        emit_rerun_if_changed(&src_dir);
    }

    // .cargo/config.toml にtargetが設定されているか確認
    let cargo_config = service_dir.join(".cargo/config.toml");
    let has_config_target = std::fs::read_to_string(&cargo_config)
        .map(|s| s.contains("[build]") && s.contains("target"))
        .unwrap_or(false);

    // cargoでサービスをビルド
    let mut cmd = Command::new("cargo");
    cmd.args(["build"]);

    // 外側の cargo ビルドの環境変数をクリア (干渉を防ぐ)
    // ジョブサーバーとビルドシステムの変数をクリアして独立したビルドにする
    for key in &[
        "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET", "CARGO_MAKEFLAGS", "__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS",
        "CARGO_BUILD_RUSTC", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
    ] {
        cmd.env_remove(key);
    }

    if !has_config_target {
        // .cargo/config.toml にtargetがない場合のみ --target を渡す
        let target_spec = find_target_spec(&service_dir);
        if let Some(target) = &target_spec {
            cmd.arg("--target").arg(target);
            println!("  Using target from JSON: {}", target);
        } else {
            let default_target = "x86_64-unknown-none";
            cmd.arg("--target").arg(default_target);
            println!("  Using default target: {}", default_target);
        }
    } else {
        println!("  Using target from .cargo/config.toml");
    }

    if service.name == "core" {
        cmd.arg("--features").arg("run_tests");
        println!("  Enabling run_tests feature for core.service");
    }

    let output = cmd
        .current_dir(&service_dir)
        .output()
        .map_err(|e| format!("Failed to run cargo: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // 末尾 4000 文字を優先表示 (エラー部分が末尾に多い)
        let stdout = String::from_utf8_lossy(&output.stdout);
        let err_tail = if stderr.len() > 2000 {
            &stderr[stderr.len() - 2000..]
        } else {
            &stderr
        };
        let out_tail = if stdout.len() > 2000 {
            &stdout[stdout.len() - 2000..]
        } else {
            &stdout
        };
        return Err(format!("Failed to build service {}: status={} STDERR={} STDOUT={}", 
            service.name, output.status, err_tail, out_tail));
    }

    // ビルド成果物を探してコピー
    let target_dir = service_dir.join("target");
    // .cargo/config.toml のターゲットかデフォルト名を使用
    let target_name: Option<String> = if has_config_target {
        Some("x86_64-swiftcore".to_string())
    } else {
        None
    };

    if let Some(binary_path) = find_built_binary(&target_dir, target_name.as_deref()) {
        let dest_name = format!("{}.service", service.name);
        let dest = output_dir.join(&dest_name);

        fs::copy(&binary_path, &dest).map_err(|e| {
            format!(
                "Failed to copy service binary to {}: {}",
                dest.display(),
                e
            )
        })?;

        println!(
            "Copied {} to {} (from {})",
            dest_name,
            output_dir.display(),
            binary_path.display()
        );
    } else {
        return Err(format!("Built binary not found for service {}", service.name));
    }

    Ok(())
}

fn find_built_binary(target_dir: &Path, target_name: Option<&str>) -> Option<PathBuf> {
    for profile in &["debug", "release"] {
        if let Some(target) = target_name {
            let dir = target_dir.join(format!("{}/{}", target, profile));
            if dir.is_dir() {
                if let Some(binary) = find_binary_in_dir(&dir) {
                    return Some(binary);
                }
            }
        }

        let dir = target_dir.join(format!("x86_64-swiftcore/{}", profile));
        if dir.is_dir() {
            if let Some(binary) = find_binary_in_dir(&dir) {
                return Some(binary);
            }
        }

        let dir = target_dir.join(profile);
        if dir.is_dir() {
            if let Some(binary) = find_binary_in_dir(&dir) {
                return Some(binary);
            }
        }
    }

    None
}
