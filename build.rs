mod builders;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use builders::{
    build_apps, build_drivers, build_newlib, build_service, build_user_libs, build_utils,
    copy_newlib_libs, create_ext2_image, create_initfs_image, parse_service_index, setup_fs_layout,
};

const BUSYBOX_URL: &str =
    "https://busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox";

/// カーネル ELF をビルドして fs/System/kernel.elf にコピーする
fn build_kernel(manifest_dir: &PathBuf, fs_dir: &PathBuf, profile: &str) {
    let kernel_crate_dir = manifest_dir.join("src/core");
    let kernel_target_dir = manifest_dir.join("target/kernel");
    let mut cmd = std::process::Command::new("cargo");
    cmd.current_dir(&kernel_crate_dir);
    // 再帰防止：カーネルがルートを dep としてビルドする際のフラグ
    cmd.env("MOCHIOS_BUILDING_KERNEL", "1");
    cmd.env("CARGO_TARGET_DIR", &kernel_target_dir);
    cmd.args(["build", "-Z", "build-std=core,alloc"]);
    if profile == "release" {
        cmd.arg("--release");
    }
    let status = cmd.status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            println!(
                "cargo:warning=kernel build exited with status {}",
                s.code().unwrap_or(-1)
            );
            return;
        }
        Err(e) => {
            println!("cargo:warning=failed to run kernel cargo build: {}", e);
            return;
        }
    }

    // kernel ELF を fs/System/kernel.elf にコピー
    // CARGO_TARGET_DIR=target/kernel を使用しているのでそちらを参照する
    let kernel_bin = kernel_target_dir
        .join("x86_64-unknown-none")
        .join(profile)
        .join("kernel");
    let system_dir = fs_dir.join("System");
    let _ = fs::create_dir_all(&system_dir);
    let dest = system_dir.join("kernel.elf");
    if kernel_bin.exists() {
        if let Err(e) = fs::copy(&kernel_bin, &dest) {
            println!(
                "cargo:warning=failed to copy kernel ELF to {}: {}",
                dest.display(),
                e
            );
        } else {
            println!("Kernel ELF copied to {}", dest.display());
        }
    } else {
        println!(
            "cargo:warning=kernel binary not found at {}",
            kernel_bin.display()
        );
    }
}

fn is_elf_binary(path: &Path) -> Result<bool, String> {
    use std::io::Read;

    let mut file =
        fs::File::open(path).map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)
        .map_err(|e| format!("Failed to read ELF magic from {}: {}", path.display(), e))?;
    Ok(magic == [0x7F, b'E', b'L', b'F'])
}

/// BusyBoxをダウンロード
fn ensure_busybox_binary(fs_dir: &Path) -> Result<(), String> {
    let binaries_dir = fs_dir.join("Binaries");
    fs::create_dir_all(&binaries_dir)
        .map_err(|e| format!("Failed to create {}: {}", binaries_dir.display(), e))?;

    let dest = binaries_dir.join("busybox.elf");
    let temp = binaries_dir.join("busybox.elf.download");

    println!("Downloading busybox from {}", BUSYBOX_URL);

    let status = std::process::Command::new("curl")
        .args(["-L", "--fail", "--silent", "--show-error", "--output"])
        .arg(&temp)
        .arg(BUSYBOX_URL)
        .status();

    match status {
        Ok(s) if s.success() => {
            if !is_elf_binary(&temp)? {
                let _ = fs::remove_file(&temp);
                return Err(format!(
                    "Downloaded file is not a valid ELF binary: {}",
                    temp.display()
                ));
            }

            if let Err(rename_err) = fs::rename(&temp, &dest) {
                fs::copy(&temp, &dest).map_err(|copy_err| {
                    format!(
                        "Failed to place busybox at {} (rename: {}, copy: {})",
                        dest.display(),
                        rename_err,
                        copy_err
                    )
                })?;
                let _ = fs::remove_file(&temp);
            }

            println!("Downloaded busybox to {}", dest.display());
            Ok(())
        }
        Ok(s) => {
            let _ = fs::remove_file(&temp);
            if dest.exists() {
                println!(
                    "cargo:warning=BusyBox download failed (status={}), using existing {}",
                    s,
                    dest.display()
                );
                Ok(())
            } else {
                Err(format!(
                    "BusyBox download failed (status={}) and no fallback file exists at {}",
                    s,
                    dest.display()
                ))
            }
        }
        Err(e) => {
            let _ = fs::remove_file(&temp);
            if dest.exists() {
                println!(
                    "cargo:warning=Failed to execute curl ({}), using existing {}",
                    e,
                    dest.display()
                );
                Ok(())
            } else {
                Err(format!(
                    "Failed to execute curl ({}) and no fallback file exists at {}",
                    e,
                    dest.display()
                ))
            }
        }
    }
}

#[allow(unused)]
fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // カーネルビルドの再帰呼び出しの場合はプレースホルダーだけ作成して終了する
    // (initfs は埋め込まず、ブートローダーが実行時にロードして BootInfo で渡す)
    if env::var("MOCHIOS_BUILDING_KERNEL").is_ok() {
        let _ = fs::write(out_dir.join("initfs.ext2"), b"");
        let _ = fs::write(out_dir.join("rootfs.ext2"), b"");
        return;
    }

    // ramfsとfsディレクトリを作成
    let ramfs_dir = manifest_dir.join("ramfs");
    let fs_dir = manifest_dir.join("fs");

    for dir in &[&ramfs_dir, &fs_dir] {
        if !dir.is_dir() {
            fs::create_dir_all(dir)
                .unwrap_or_else(|_| panic!("Failed to create directory: {}", dir.display()));
        }
    }

    // fsの標準ディレクトリレイアウトを作成
    let resources_src = manifest_dir.join("src/resources");
    setup_fs_layout(&fs_dir, &resources_src)
        .unwrap_or_else(|e| println!("cargo:warning=setup_fs_layout failed: {}", e));

    // newlibのインストールディレクトリを取得
    let target = env::var("TARGET").unwrap_or("x86_64-unknown-uefi".to_string());
    let profile = env::var("PROFILE").unwrap_or("debug".to_string());
    let target_dir = PathBuf::from(env::var("CARGO_TARGET_DIR").unwrap_or("target".to_string()));

    // カーネル ELF をビルド
    build_kernel(&manifest_dir, &fs_dir, &profile);

    // newlibのビルド
    let newlib_src_dir = manifest_dir.join("src/lib");
    if !newlib_src_dir.exists() {
        panic!("Newlib source not found at {}", newlib_src_dir.display());
    }
    build_newlib(&newlib_src_dir);

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
    copy_newlib_libs(&libc_dir, &ramfs_dir.join("Libraries"))
        .expect("cargo:warning=Failed to copy newlib libs to ramfs/Libraries");
    copy_newlib_libs(&libc_dir, &fs_dir.join("Libraries"))
        .expect("cargo:warning=Failed to copy newlib libs to fs/Libraries");

    // libgcc_sをfs/Librariesにコピー
    if let Ok(out) = std::process::Command::new("gcc")
        .arg("-print-file-name=libgcc_s.so.1")
        .output()
    {
        if out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            use std::path::Path;
            let libs_dir = fs_dir.join("Libraries");
            let _ = fs::create_dir_all(&libs_dir);
            if path != "libgcc_s.so.1" && Path::new(&path).exists() {
                let dest = libs_dir.join("libgcc_s.so.1");
                let _ = fs::copy(&path, &dest);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::symlink;
                    let link = libs_dir.join("libgcc_s.so");
                    if !link.exists() {
                        let _ = symlink("libgcc_s.so.1", &link);
                    }
                }
                println!("Copied libgcc_s to fs/Libraries: {}", path);
            } else {
                let candidates = [
                    "/usr/lib/x86_64-linux-gnu/libgcc_s.so.1",
                    "/lib/x86_64-linux-gnu/libgcc_s.so.1",
                    "/usr/lib64/libgcc_s.so.1",
                    "/lib64/libgcc_s.so.1",
                ];
                for c in &candidates {
                    if Path::new(c).exists() {
                        let _ = fs::copy(c, libs_dir.join("libgcc_s.so.1"));
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::symlink;
                            let link = libs_dir.join("libgcc_s.so");
                            if !link.exists() {
                                let _ = symlink("libgcc_s.so.1", &link);
                            }
                        }
                        println!("Copied libgcc_s to fs/Libraries from {}", c);
                        break;
                    }
                }
            }
        } else {
            println!("gcc returned non-zero when locating libgcc_s");
        }
    } else {
        println!("Failed to run gcc to locate libgcc_s");
    }

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
            println!(
                "cargo:warning=Failed to build service {}: {}",
                service.name, e
            );
        }
    }

    // アプリケーションをビルド
    let apps_dir = manifest_dir.join("src/apps");
    if apps_dir.is_dir() {
        println!("Building test applications");
        build_apps(&apps_dir, &fs_dir, "elf");
    }

    // ユーティリティコマンドをビルド
    let utils_dir = manifest_dir.join("src/utils");
    let binaries_dir = fs_dir.join("Binaries");
    if utils_dir.is_dir() {
        println!("Building utility commands");
        build_utils(&utils_dir, &binaries_dir);
    }

    ensure_busybox_binary(&fs_dir).expect("Failed to ensure busybox binary");

    // ドライバをビルド
    let drivers_dir = manifest_dir.join("src/drivers");
    let drivers_binaries_dir = binaries_dir.join("drivers");
    let driver_autostart_entries = if drivers_dir.is_dir() {
        println!("Building drivers");
        build_drivers(&drivers_dir, &drivers_binaries_dir)
    } else {
        Vec::new()
    };

    // driver.service が参照する自動起動ドライバ一覧を生成
    let driver_autostart_path = fs_dir.join("Config").join("drivers.list");
    match fs::write(&driver_autostart_path, driver_autostart_entries.join("\n")) {
        Ok(_) => println!("Generated {}", driver_autostart_path.display()),
        Err(e) => println!(
            "cargo:warning=Failed to write {}: {}",
            driver_autostart_path.display(),
            e
        ),
    }

    // initfs イメージを生成
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
