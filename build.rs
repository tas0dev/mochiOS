use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let apps_dir = manifest_dir.join("src/apps");
    let services_dir = manifest_dir.join("src/services");

    // fsディレクトリを使用（プロジェクトルート直下）
    let initfs_dir = manifest_dir.join("fs");

    // initfsディレクトリが存在しない場合、作成
    if !initfs_dir.is_dir() {
        fs::create_dir_all(&initfs_dir).expect(&format!(
            "Failed to create initfs directory: {}",
            initfs_dir.display()
        ));

        println!("created initfs directory at {}", initfs_dir.display());
    }

    // appsディレクトリが存在する場合、アプリをビルド
    if apps_dir.is_dir() {
        build_apps(&apps_dir, &initfs_dir, "elf");
    }

    // servicesディレクトリが存在する場合、サービスをビルド
    if services_dir.is_dir() {
        build_apps(&services_dir, &initfs_dir, "service");
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let image_path = out_dir.join("initfs.ext2");

    emit_rerun_if_changed(&initfs_dir);

    let status = Command::new("mke2fs")
        .args(["-t", "ext2", "-b", "4096", "-m", "0", "-L", "initfs", "-d"])
        .arg(&initfs_dir)
        .arg(&image_path)
        .arg("4096")
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            panic!("mke2fs failed while generating initfs.ext2");
        }
        Err(e) => {
            panic!("failed to execute mke2fs: {e}. Please install e2fsprogs (mke2fs).");
        }
    }
}

fn build_apps(apps_dir: &Path, initfs_dir: &Path, extension: &str) {
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

        // cargoでアプリをビルド（出力をキャプチャ）
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--release"]);

        // カスタムターゲットが見つかった場合は指定
        if let Some(target) = &target_spec {
            cmd.arg("--target").arg(target);
            println!("  Using target: {}", target);
        }

        let output = cmd.current_dir(&path).output();

        match output {
            Ok(output) => {
                if output.status.success() {
                    // ビルド成果物を探す
                    let target_dir = path.join("target");
                    let target_name = target_spec.as_ref()
                        .and_then(|p| Path::new(p).file_stem())
                        .map(|s| s.to_string_lossy().to_string());

                    if let Some(elf_path) = find_built_binary(&target_dir, target_name.as_deref()) {
                        let dest_name = format!("{}.{}", app_name, extension);
                        let dest = initfs_dir.join(&dest_name);
                        if let Err(e) = fs::copy(&elf_path, &dest) {
                            println!("cargo:warning=Failed to copy {} to initfs: {}", dest_name, e);
                        } else {
                            println!("Copied {} to initfs (from {})", dest_name, elf_path.display());
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
                println!("cargo:warning=Failed to execute cargo for {}: {}", app_name, e);
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

fn find_target_spec(app_dir: &Path) -> Option<String> {
    // .jsonファイルを探す（x86_64-*.json など）
    if let Ok(entries) = fs::read_dir(app_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(filename) = path.file_name() {
                    let filename_str = filename.to_string_lossy();
                    if filename_str.ends_with(".json") && filename_str.starts_with("x86_64-") {
                        // 絶対パスを返す
                        return path.to_str().map(|s| s.to_string());
                    }
                }
            }
        }
    }

    // .cargo/config.tomlでデフォルトターゲットが指定されている可能性もあるが、
    // とりあえずjsonファイルの検出のみ
    None
}

fn find_binary_in_dir(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let filename = path.file_name()?.to_string_lossy();
            // 実行可能ファイルっぽいものを探す（拡張子なし、.so, .dなどを除外）
            if !filename.starts_with("lib")
                && !filename.ends_with(".d")
                && !filename.ends_with(".rlib")
                && !filename.ends_with(".so")
                && !filename.contains(".") {
                return Some(path);
            }
        }
    }

    None
}

fn emit_rerun_if_changed(path: &Path) {
    // targetディレクトリは除外
    if let Some(file_name) = path.file_name() {
        if file_name == "target" || file_name == ".git" {
            return;
        }
    }

    if let Ok(metadata) = fs::metadata(path) {
        if metadata.is_file() {
            println!("cargo:rerun-if-changed={}", path.display());
        } else if metadata.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    emit_rerun_if_changed(&entry.path());
                }
            }
        }
    }
}
