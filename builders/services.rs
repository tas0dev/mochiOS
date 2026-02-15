use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::utils::{emit_rerun_if_changed, find_binary_in_dir, find_target_spec};

/// サービスインデックスの情報
#[derive(Debug, Clone)]
pub struct ServiceEntry {
    pub name: String,
    pub dir: String,
    pub fs_type: String,
    pub description: String,
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

    for line in content.lines() {
        let line = line.trim();

        // [core.service.NAME] を解析
        if line.starts_with("[core.service.") && line.ends_with(']') {
            // 前のサービスを保存
            if !current_service.is_empty() {
                services.push(ServiceEntry {
                    name: current_service.clone(),
                    dir: current_dir.clone(),
                    fs_type: current_fs.clone(),
                    description: current_desc.clone(),
                });
            }

            // 新しいサービス名を取得
            let start = "[core.service.".len();
            let end = line.len() - 1;
            current_service = line[start..end].to_string();
            current_dir.clear();
            current_fs.clear();
            current_desc.clear();
        } else if line.starts_with("dir = ") {
            current_dir = line["dir = ".len()..]
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
        } else if line.starts_with("fs = ") {
            current_fs = line["fs = ".len()..]
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
        } else if line.starts_with("description = ") {
            current_desc = line["description = ".len()..]
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
        }
    }

    // 最後のサービスを保存
    if !current_service.is_empty() {
        services.push(ServiceEntry {
            name: current_service,
            dir: current_dir,
            fs_type: current_fs,
            description: current_desc,
        });
    }

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

    // カスタムターゲットファイルを探す
    let target_spec = find_target_spec(&service_dir);

    // cargoでサービスをビルド
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release"]);

    if let Some(target) = &target_spec {
        cmd.arg("--target").arg(target);
        println!("  Using target: {}", target);
    } else {
        let default_target = "x86_64-unknown-none";
        cmd.arg("--target").arg(default_target);
        println!("  Using default target: {}", default_target);
    }

    let output = cmd
        .current_dir(&service_dir)
        .output()
        .map_err(|e| format!("Failed to run cargo: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to build service {}: {}", service.name, stderr));
    }

    // ビルド成果物を探してコピー
    let target_dir = service_dir.join("target");
    let target_name = if let Some(p) = &target_spec {
        Path::new(p)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
    } else {
        Some("x86_64-unknown-none".to_string())
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
    if let Some(target) = target_name {
        let custom_target = target_dir.join(format!("{}/release", target));
        if custom_target.is_dir() {
            if let Some(binary) = find_binary_in_dir(&custom_target) {
                return Some(binary);
            }
        }
    }

    let custom_target = target_dir.join("x86_64-swiftcore/release");
    if custom_target.is_dir() {
        if let Some(binary) = find_binary_in_dir(&custom_target) {
            return Some(binary);
        }
    }

    let release_dir = target_dir.join("release");
    if release_dir.is_dir() {
        if let Some(binary) = find_binary_in_dir(&release_dir) {
            return Some(binary);
        }
    }

    None
}
