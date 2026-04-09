use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::utils::{emit_rerun_if_changed, find_binary_in_dir, find_target_spec};

/// ドライバクレート (`src/drivers/*`) をビルドして `fs/Binaries/drivers/*.elf` に配置する。
///
/// 戻り値は `driver.service` が起動するドライバパス（例: `Binaries/drivers/usb3.0.elf`）。
pub fn build_drivers(drivers_dir: &Path, output_dir: &Path) -> Vec<String> {
    println!("cargo:rerun-if-changed={}", drivers_dir.display());

    let mut autostart_entries = Vec::new();

    let entries = match fs::read_dir(drivers_dir) {
        Ok(entries) => entries,
        Err(_) => return autostart_entries,
    };

    if let Err(e) = fs::create_dir_all(output_dir) {
        println!(
            "cargo:warning=Failed to create drivers output dir {}: {}",
            output_dir.display(),
            e
        );
        return autostart_entries;
    }

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let cargo_toml = path.join("Cargo.toml");
        if !cargo_toml.exists() {
            continue;
        }

        let driver_dir_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let driver_output_name = normalize_driver_name(&driver_dir_name);
        let expected_bin_name =
            parse_driver_binary_name(&cargo_toml).or_else(|| Some(driver_dir_name.clone()));

        println!(
            "Building driver: {} -> {}.elf",
            driver_dir_name, driver_output_name
        );

        println!("cargo:rerun-if-changed={}", cargo_toml.display());
        let src_dir = path.join("src");
        if src_dir.is_dir() {
            emit_rerun_if_changed(&src_dir);
        }

        let target_spec = find_target_spec(&path);
        let mut uses_json_target = target_spec
            .as_deref()
            .map(|t| t.ends_with(".json"))
            .unwrap_or(false);

        let cargo_config = path.join(".cargo/config.toml");
        let cargo_config_text = fs::read_to_string(&cargo_config).ok();
        let config_target = cargo_config_text
            .as_deref()
            .and_then(parse_build_target_from_cargo_config_text);
        let has_config_target = config_target.is_some();
        let config_uses_json_target = cargo_config_text
            .as_deref()
            .and_then(parse_build_target_from_cargo_config_text)
            .map(|target| target.ends_with(".json"))
            .unwrap_or(false);
        uses_json_target = uses_json_target || config_uses_json_target;

        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--release"]);
        if uses_json_target {
            cmd.args(["-Z", "json-target-spec"]);
        }

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

        if has_config_target {
            println!("  Using target from .cargo/config.toml");
        } else if let Some(target) = &target_spec {
            cmd.arg("--target").arg(target);
            println!("  Using target: {}", target);
        } else {
            let default_target = "x86_64-unknown-none";
            cmd.arg("--target").arg(default_target);
            println!("  Using default target: {}", default_target);
        }

        let output = cmd.current_dir(&path).output();

        match output {
            Ok(output) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let err_tail = tail_from_char_boundary(&stderr, 4000);
                    let out_tail = tail_from_char_boundary(&stdout, 2000);
                    panic!(
                        "Failed to build driver {}: status={} STDERR={} STDOUT={}",
                        driver_dir_name, output.status, err_tail, out_tail
                    );
                }

                let target_dir = path.join("target");
                let target_name = if has_config_target {
                    config_target
                        .or_else(|| {
                            target_spec
                                .as_ref()
                                .and_then(|p| Path::new(p).file_stem())
                                .map(|s| s.to_string_lossy().to_string())
                        })
                        .or_else(|| Some("x86_64-unknown-none".to_string()))
                } else if let Some(p) = &target_spec {
                    Path::new(p)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                } else {
                    Some("x86_64-unknown-none".to_string())
                };

                if let Some(elf_path) = find_built_binary(
                    &target_dir,
                    target_name.as_deref(),
                    expected_bin_name.as_deref(),
                ) {
                    let dest_name = format!("{}.elf", driver_output_name);
                    let dest = output_dir.join(&dest_name);
                    if let Err(e) = fs::copy(&elf_path, &dest) {
                        println!(
                            "cargo:warning=Failed to copy {} to {}: {}",
                            elf_path.display(),
                            dest.display(),
                            e
                        );
                    } else {
                        println!(
                            "Copied {} to {} (from {})",
                            dest_name,
                            output_dir.display(),
                            elf_path.display()
                        );
                        autostart_entries.push(format!("/Binaries/drivers/{}", dest_name));
                    }
                } else {
                    panic!("Built driver binary not found for {}", driver_dir_name);
                }
            }
            Err(e) => {
                panic!(
                    "Failed to execute cargo for driver {}: {}",
                    driver_dir_name, e
                );
            }
        }
    }

    autostart_entries.sort();
    autostart_entries
}

fn tail_from_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }

    let target_start = s.len().saturating_sub(max_bytes);
    let byte_index = s
        .char_indices()
        .rev()
        .find(|(idx, _)| *idx <= target_start)
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    &s[byte_index..]
}

fn parse_build_target_from_cargo_config_text(content: &str) -> Option<String> {
    let mut in_build = false;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            in_build = line == "[build]";
            continue;
        }

        if !in_build {
            continue;
        }

        let Some((key_raw, value_raw)) = line.split_once('=') else {
            continue;
        };
        if key_raw.trim() != "target" {
            continue;
        }

        let mut comment_stripped = String::with_capacity(value_raw.len());
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        for ch in value_raw.chars() {
            match ch {
                '\'' if !in_double_quote => {
                    in_single_quote = !in_single_quote;
                    comment_stripped.push(ch);
                }
                '"' if !in_single_quote => {
                    in_double_quote = !in_double_quote;
                    comment_stripped.push(ch);
                }
                '#' if !in_single_quote && !in_double_quote => break,
                _ => comment_stripped.push(ch),
            }
        }

        let value_no_comment = comment_stripped.trim();
        let value = value_no_comment.trim_matches('"').trim_matches('\'').trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    None
}

fn normalize_driver_name(driver_dir_name: &str) -> String {
    // 例: usb3_0 -> usb3.0
    driver_dir_name.replace('_', ".")
}

fn parse_driver_binary_name(cargo_toml: &Path) -> Option<String> {
    let content = fs::read_to_string(cargo_toml).ok()?;
    let mut in_bin = false;
    let mut in_package = false;
    let mut package_name: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_bin = line == "[[bin]]";
            in_package = line == "[package]";
            continue;
        }

        let Some((lhs, rhs)) = line.split_once('=') else {
            continue;
        };
        if lhs.trim() != "name" {
            continue;
        }

        let name = rhs
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }

        if in_bin {
            return Some(name);
        }
        if in_package && package_name.is_none() {
            package_name = Some(name);
        }
    }

    package_name
}

fn find_binary_in_dir_prefer(dir: &Path, preferred: Option<&str>) -> Option<PathBuf> {
    if let Some(name) = preferred {
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        let alt = dir.join(name.replace('-', "_"));
        if alt.is_file() {
            return Some(alt);
        }
    }
    find_binary_in_dir(dir)
}

fn find_built_binary(
    target_dir: &Path,
    target_name: Option<&str>,
    preferred_bin: Option<&str>,
) -> Option<PathBuf> {
    if let Some(target) = target_name {
        let custom_target = target_dir.join(format!("{}/release", target));
        if custom_target.is_dir() {
            if let Some(binary) = find_binary_in_dir_prefer(&custom_target, preferred_bin) {
                return Some(binary);
            }
        }
    }

    let custom_target = target_dir.join("x86_64-mochios/release");
    if custom_target.is_dir() {
        if let Some(binary) = find_binary_in_dir_prefer(&custom_target, preferred_bin) {
            return Some(binary);
        }
    }

    let release_dir = target_dir.join("release");
    if release_dir.is_dir() {
        if let Some(binary) = find_binary_in_dir_prefer(&release_dir, preferred_bin) {
            return Some(binary);
        }
    }

    None
}
