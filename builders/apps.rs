use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::utils::{emit_rerun_if_changed, find_binary_in_dir, find_target_spec};

/// アプリケーションをビルドして指定ディレクトリにコピー
pub fn build_apps(apps_dir: &Path, output_dir: &Path, _extension: &str) {
    println!("cargo:rerun-if-changed={}", apps_dir.display());

    let entries = match fs::read_dir(apps_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    // START_TEST_APP環境変数をチェック
    let run_tests = std::env::var("START_TEST_APP")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let app_name = path.file_name().unwrap().to_string_lossy().to_string();

        // testsディレクトリはSTART_TEST_APP=trueの場合のみビルド
        if app_name == "tests" && !run_tests {
            println!("Skipping tests app (START_TEST_APP not enabled)");
            continue;
        }

        let cargo_toml = path.join("Cargo.toml");
        if !cargo_toml.exists() {
            continue;
        }

        println!("Building app: {}", app_name);

        // アプリのソースファイルを明示的に監視
        println!("cargo:rerun-if-changed={}", cargo_toml.display());
        let src_dir = path.join("src");
        if src_dir.is_dir() {
            emit_rerun_if_changed(&src_dir);
        }
        let resources_dir = path.join("resources");
        if resources_dir.is_dir() {
            emit_rerun_if_changed(&resources_dir);
        }

        // カスタムターゲットファイルを探す（アプリディレクトリ内の .json を優先）
        let target_spec = find_target_spec(&path);
        let uses_json_target = target_spec
            .as_deref()
            .map(|t| t.ends_with(".json"))
            .unwrap_or(false);

        // .cargo/config.toml にtargetが設定されているか確認
        let cargo_config = path.join(".cargo/config.toml");
        let cargo_config_text = fs::read_to_string(&cargo_config).ok();
        let has_config_target = cargo_config_text
            .as_deref()
            .map(|s| s.contains("[build]") && s.contains("target"))
            .unwrap_or(false);
        let config_uses_json_target = cargo_config_text
            .as_deref()
            .map(|s| s.contains(".json"))
            .unwrap_or(false);
        let uses_json_target = uses_json_target || config_uses_json_target;

        // cargoでアプリをビルド
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--release"]);
        if uses_json_target {
            cmd.args(["-Z", "json-target-spec"]);
        }

        // 外側のビルド環境変数をクリアして干渉を防ぐ
        for key in &[
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_TARGET_DIR",
            "CARGO_BUILD_TARGET",
            "CARGO_MAKEFLAGS",
            "__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS",
            "CARGO_BUILD_RUSTC",
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
        ] {
            cmd.env_remove(key);
        }

        // .cargo/config.toml にtargetがある場合は --target を渡さない
        if has_config_target {
            println!("  Using target from .cargo/config.toml");
        } else if let Some(target) = &target_spec {
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
                    let target_name = if has_config_target {
                        Some("x86_64-mochios".to_string())
                    } else if let Some(p) = &target_spec {
                        Path::new(p)
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                    } else {
                        Some("x86_64-unknown-none".to_string())
                    };

                    if let Some(elf_path) = find_built_binary(&target_dir, target_name.as_deref()) {
                        let app_bundle_dir = output_dir.join(format!("{}.app", app_name));
                        if let Err(e) = fs::create_dir_all(&app_bundle_dir) {
                            println!(
                                "cargo:warning=Failed to create app bundle dir for {}: {}",
                                app_name, e
                            );
                            continue;
                        }

                        let dest = app_bundle_dir.join("entry.elf");
                        if let Err(e) = fs::copy(&elf_path, &dest) {
                            println!(
                                "cargo:warning=Failed to copy app entry for {}: {}",
                                app_name, e
                            );
                        } else {
                            println!(
                                "Copied {} entry to {} (from {})",
                                app_name,
                                dest.display(),
                                elf_path.display()
                            );
                        }

                        let about_src = path.join("about.toml");
                        let about_dest = app_bundle_dir.join("about.toml");
                        if about_src.exists() {
                            if let Err(e) = fs::copy(&about_src, &about_dest) {
                                println!(
                                    "cargo:warning=Failed to copy about.toml for {}: {}",
                                    app_name, e
                                );
                            }
                        } else {
                            println!(
                                "cargo:warning=about.toml not found for app {} ({})",
                                app_name,
                                about_src.display()
                            );
                        }

                        for icon_file in ["icon.png", "icon.jpeg", "icon.jpg"] {
                            let icon_src = path.join(icon_file);
                            if icon_src.exists() {
                                let icon_dest = app_bundle_dir.join(icon_file);
                                if let Err(e) = fs::copy(&icon_src, &icon_dest) {
                                    println!(
                                        "cargo:warning=Failed to copy {} for {}: {}",
                                        icon_file, app_name, e
                                    );
                                }
                                break;
                            }
                        }

                        for res_dir_name in ["resources", "resource"] {
                            let res_src = path.join(res_dir_name);
                            if res_src.is_dir() {
                                let res_dest = app_bundle_dir.join("resources");
                                if let Err(e) = copy_dir_recursive(&res_src, &res_dest) {
                                    println!(
                                        "cargo:warning=Failed to copy resources for {}: {}",
                                        app_name, e
                                    );
                                }
                                break;
                            }
                        }

                        if let Some(fs_root) = output_dir.parent() {
                            let app_service_dir =
                                fs_root.join("Libraries").join("AppService").join(&app_name);
                            if let Err(e) = fs::create_dir_all(&app_service_dir) {
                                println!(
                                    "cargo:warning=Failed to create app service dir for {}: {}",
                                    app_name, e
                                );
                            }
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

/// ユーティリティコマンド (`src/utils/`) をビルドして `output_dir` に `{name}.elf` としてコピー
pub fn build_utils(utils_dir: &Path, output_dir: &Path) {
    println!("cargo:rerun-if-changed={}", utils_dir.display());
    let cargo_toml = utils_dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return;
    }

    println!("cargo:rerun-if-changed={}", cargo_toml.display());
    let src_dir = utils_dir.join("src");
    if src_dir.is_dir() {
        emit_rerun_if_changed(&src_dir);
    }

    if let Err(e) = fs::create_dir_all(output_dir) {
        println!("cargo:warning=Failed to create Binaries dir: {}", e);
        return;
    }

    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release", "-Z", "json-target-spec"]);

    for key in &[
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_MAKEFLAGS",
        "__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS",
        "CARGO_BUILD_RUSTC",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ] {
        cmd.env_remove(key);
    }

    println!("Building utils from {}", utils_dir.display());
    let output = cmd.current_dir(utils_dir).output();

    match output {
        Ok(output) => {
            if !output.status.success() {
                println!("cargo:warning=Failed to build utils");
                let stderr = String::from_utf8_lossy(&output.stderr);
                for line in stderr.lines().take(20) {
                    println!("cargo:warning=  {}", line);
                }
                return;
            }
            // 全てのELFバイナリを探してコピー
            let release_dir = utils_dir.join("target/x86_64-mochios/release");
            let binaries = find_all_binaries(&release_dir);
            if binaries.is_empty() {
                println!(
                    "cargo:warning=No binaries found in {}",
                    release_dir.display()
                );
            }
            for elf_path in binaries {
                let name = elf_path.file_name().unwrap().to_string_lossy();
                let dest = output_dir.join(format!("{}.elf", name));
                if let Err(e) = fs::copy(&elf_path, &dest) {
                    println!("cargo:warning=Failed to copy {}.elf: {}", name, e);
                } else {
                    println!("Copied {}.elf to {}", name, output_dir.display());
                }
            }
        }
        Err(e) => {
            println!("cargo:warning=Failed to execute cargo for utils: {}", e);
        }
    }
}

/// ディレクトリ内の全ELFバイナリを返す（拡張子なし・libでない・.dでない）
fn find_all_binaries(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return result,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let filename = path.file_name().unwrap().to_string_lossy().to_string();
        if !filename.starts_with("lib")
            && !filename.ends_with(".d")
            && !filename.ends_with(".rlib")
            && !filename.ends_with(".so")
            && !filename.contains('.')
            && is_elf(&path)
        {
            result.push(path);
        }
    }
    result
}

/// ファイルがELFマジックバイトで始まるか確認
fn is_elf(path: &Path) -> bool {
    if let Ok(mut f) = fs::File::open(path) {
        use std::io::Read;
        let mut magic = [0u8; 4];
        if f.read_exact(&mut magic).is_ok() {
            return magic == [0x7f, b'E', b'L', b'F'];
        }
    }
    false
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

    // x86_64-mochios/release/ を優先的に探す
    let custom_target = target_dir.join("x86_64-mochios/release");
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

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create {}: {}", dst.display(), e))?;
    for entry in
        fs::read_dir(src).map_err(|e| format!("Failed to read {}: {}", src.display(), e))?
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
        }
    }
    Ok(())
}
