use num_cpus;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Well-known paths where cross-compiler toolchains are commonly installed.
const EXTRA_TOOL_PATHS: &[&str] = &[
    "/home/linuxbrew/.linuxbrew/bin",
    "/usr/local/bin",
    "/opt/homebrew/bin",
];

fn tool_exists(name: &str) -> bool {
    // First try via PATH as-is
    if Command::new(name).arg("--version").output().is_ok() {
        return true;
    }
    // Then probe well-known locations (needed when launched from IDEs that
    // inherit a stripped-down environment without linuxbrew in PATH)
    EXTRA_TOOL_PATHS.iter().any(|dir| {
        let full = Path::new(dir).join(name);
        Command::new(&full).arg("--version").output().is_ok()
    })
}

/// Return the full path to a tool, checking well-known locations before name-only fallback.
fn find_tool(name: &str) -> String {
    if Command::new(name).arg("--version").output().is_ok() {
        return name.to_string();
    }
    for dir in EXTRA_TOOL_PATHS {
        let full = Path::new(dir).join(name);
        if Command::new(&full).arg("--version").output().is_ok() {
            return full.to_string_lossy().into_owned();
        }
    }
    name.to_string()
}

fn apply_cross_target_tools(cmd: &mut Command) {
    cmd.env("CC_FOR_TARGET", find_tool("x86_64-elf-gcc"))
        .env("CXX_FOR_TARGET", find_tool("x86_64-elf-g++"))
        .env("AR_FOR_TARGET", find_tool("x86_64-elf-ar"))
        .env("AS_FOR_TARGET", find_tool("x86_64-elf-as"))
        .env("LD_FOR_TARGET", find_tool("x86_64-elf-ld"))
        .env("NM_FOR_TARGET", find_tool("x86_64-elf-nm"))
        .env("RANLIB_FOR_TARGET", find_tool("x86_64-elf-ranlib"))
        .env("STRIP_FOR_TARGET", find_tool("x86_64-elf-strip"))
        .env("OBJCOPY_FOR_TARGET", find_tool("x86_64-elf-objcopy"))
        .env("OBJDUMP_FOR_TARGET", find_tool("x86_64-elf-objdump"))
        .env("READELF_FOR_TARGET", find_tool("x86_64-elf-readelf"));
}

fn apply_host_target_tool_fallback(cmd: &mut Command) {
    cmd.env("CC_FOR_TARGET", "gcc")
        .env("CXX_FOR_TARGET", "g++")
        .env("AR_FOR_TARGET", "ar")
        .env("AS_FOR_TARGET", "as")
        .env("LD_FOR_TARGET", "ld")
        .env("NM_FOR_TARGET", "nm")
        .env("RANLIB_FOR_TARGET", "ranlib")
        .env("STRIP_FOR_TARGET", "strip")
        .env("OBJCOPY_FOR_TARGET", "objcopy")
        .env("OBJDUMP_FOR_TARGET", "objdump")
        .env("READELF_FOR_TARGET", "readelf");
}

pub fn build_newlib(src_dir: &Path) {
    let target = env::var("TARGET").expect("TARGET not set");
    let profile = env::var("PROFILE").expect("PROFILE not set");

    let target_dir = PathBuf::from(env::var("CARGO_TARGET_DIR").unwrap_or("target".to_string()));
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Resolve absolute target dir
    let abs_target_dir = if target_dir.is_absolute() {
        target_dir
    } else {
        manifest_dir.join(target_dir)
    };

    let build_base_dir = abs_target_dir.join(&target).join(&profile);

    let install_dir = build_base_dir.join("newlib_install");
    let build_dir = build_base_dir.join("newlib_build");
    let use_host_target_tool_fallback = !tool_exists("x86_64-elf-gcc");

    if use_host_target_tool_fallback {
        println!(
            "cargo:warning=x86_64-elf-gcc not found; using host gcc/binutils as target tool fallback"
        );
    }

    // Check if libc.a exists in the install location
    if install_dir.join("x86_64-elf/lib/libc.a").exists() {
        println!("newlib already built, skipping");
        return;
    }

    if !build_dir.exists() {
        fs::create_dir_all(&build_dir).expect("Failed to create newlib build dir");
    }

    // Configure (if Makefile doesn't exist)
    if !build_dir.join("Makefile").exists() {
        println!("Configuring newlib...");

        let configure_script = src_dir.join("configure");
        if !configure_script.exists() {
            panic!(
                "configure script not found at {}",
                configure_script.display()
            );
        }

        let abs_configure = configure_script.canonicalize().unwrap();

        let mut configure_cmd = Command::new(abs_configure);
        configure_cmd
            .current_dir(&build_dir)
            .arg(format!("--target={}", "x86_64-elf"))
            .arg(format!("--prefix={}", install_dir.display()))
            .arg("--disable-multilib");
        if use_host_target_tool_fallback {
            apply_host_target_tool_fallback(&mut configure_cmd);
        } else {
            apply_cross_target_tools(&mut configure_cmd);
        }

        let status = configure_cmd
            .status()
            .expect("Failed to execute newlib configure");

        if !status.success() {
            let _ = fs::remove_dir_all(&build_dir);
            panic!("Newlib configure failed. Build directory cleaned up.");
        }
    }

    let cpu_cores = num_cpus::get();
    let make_j = format!("-j{}", cpu_cores);

    println!("Building newlib...");

    let mut make_cmd = Command::new("make");
    make_cmd.current_dir(&build_dir).arg(make_j);
    if use_host_target_tool_fallback {
        apply_host_target_tool_fallback(&mut make_cmd);
    } else {
        apply_cross_target_tools(&mut make_cmd);
    }

    let status = make_cmd.status().expect("Failed to execute newlib make");

    if !status.success() {
        let _ = fs::remove_dir_all(&build_dir);
        panic!("Newlib make failed. Build directory cleaned up. Please try again.");
    }

    println!("Installing newlib...");

    let mut make_install_cmd = Command::new("make");
    make_install_cmd.current_dir(&build_dir).arg("install");
    if use_host_target_tool_fallback {
        apply_host_target_tool_fallback(&mut make_install_cmd);
    } else {
        apply_cross_target_tools(&mut make_install_cmd);
    }

    let status = make_install_cmd
        .status()
        .expect("Failed to execute newlib make install");

    if !status.success() {
        let _ = fs::remove_dir_all(&build_dir);
        panic!("Newlib make install failed. Build directory cleaned up.");
    }
}

pub fn build_user_libs(user_dir: &Path, libc_dir: &Path) {
    if !libc_dir.exists() {
        fs::create_dir_all(libc_dir).expect("Failed to create libc dir");
    }

    let crt_src = user_dir.join("crt.rs");
    let lib_src = user_dir.join("lib.rs");
    let crt_obj = libc_dir.join("crt0.o");
    let libc_a = libc_dir.join("libc.a");
    let libg_a = libc_dir.join("libg.a");
    // 元の newlib libc.a を一度だけ保存しておく。以降のマージはここを起点にする。
    let libc_base_a = libc_dir.join("libc_newlib_base.a");
    let glue_lib = libc_dir.join("libuserglue.a");

    // 初回のみ: 元の newlib libc.a をバックアップ
    if !libc_base_a.exists() && libc_a.exists() {
        fs::copy(&libc_a, &libc_base_a).expect("Failed to save libc_newlib_base.a");
    }

    // ソースが変更されていなければスキップ (crt0.o が存在する場合のみ)
    if libc_base_a.exists() && libc_a.exists() && crt_obj.exists() {
        let libc_mtime = libc_a.metadata().and_then(|m| m.modified()).ok();
        let lib_mtime = lib_src.metadata().and_then(|m| m.modified()).ok();
        let crt_mtime = crt_src.metadata().and_then(|m| m.modified()).ok();
        // Check all .rs files in user_dir for changes
        let newest_src = fs::read_dir(user_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "rs").unwrap_or(false))
            .filter_map(|e| e.metadata().and_then(|m| m.modified()).ok())
            .max();
        if let (Some(libc_t), Some(lib_t), Some(crt_t)) = (libc_mtime, lib_mtime, crt_mtime) {
            let newest = [lib_t, crt_t].into_iter().chain(newest_src).max().unwrap_or(lib_t);
            if libc_t > newest {
                println!("user libs up to date, skipping");
                return;
            }
        }
    }

    println!("Building user libs...");

    // 1. crt0.o のビルド
    let status = Command::new("rustc")
        .args(["--emit", "obj"])
        .args(["--crate-type", "lib"])
        .args(["--edition", "2021"])
        .args(["--target", "x86_64-unknown-none"])
        .args(["-o", crt_obj.to_str().unwrap()])
        .arg(&crt_src)
        .status()
        .expect("Failed to build crt0.o");
    if !status.success() {
        panic!("Failed to build crt0.o");
    }

    // 2. libuserglue.a のビルド
    let status = Command::new("rustc")
        .args(["--crate-type", "staticlib"])
        .args(["--edition", "2021"])
        .args(["--target", "x86_64-unknown-none"])
        .args(["-C", "panic=abort"])
        .args(["-o", glue_lib.to_str().unwrap()])
        .arg(&lib_src)
        .status()
        .expect("Failed to build libuserglue.a");
    if !status.success() {
        panic!("Failed to build libuserglue.a");
    }

    // 3. libc_newlib_base.a + libuserglue.a → libc.a へマージ
    //
    // 方針: libc.a を直接上書きせず、temp ファイルに書いてから rename する。
    // 中断してもオリジナルの libc_newlib_base.a は常に無傷で残る。
    let merge_dir = libc_dir.join("merge_tmp");
    if merge_dir.exists() {
        fs::remove_dir_all(&merge_dir).unwrap();
    }
    fs::create_dir(&merge_dir).unwrap();

    // libuserglue.a のオブジェクトだけを展開 (libc.a は触らない)
    let status = Command::new("ar")
        .current_dir(&merge_dir)
        .arg("x")
        .arg(&glue_lib)
        .status()
        .expect("Failed to extract libuserglue.a");
    if !status.success() {
        panic!("ar x libuserglue.a failed");
    }

    // ベースのコピーに userglue オブジェクトを追加 (ar q = quick append, インデックスは後で再構築)
    let libc_tmp = libc_dir.join("libc_merged_tmp.a");
    let base_src = if libc_base_a.exists() { &libc_base_a } else { &libc_a };
    fs::copy(base_src, &libc_tmp).expect("Failed to copy libc base to temp");

    let status = Command::new("sh")
        .current_dir(&merge_dir)
        .arg("-c")
        .arg(format!("ar q {} *.o", libc_tmp.to_str().unwrap()))
        .status()
        .expect("Failed to append objects to libc temp");
    if !status.success() {
        panic!("ar q libc_merged_tmp.a failed");
    }

    // シンボルインデックスを再構築
    let status = Command::new("ranlib")
        .arg(&libc_tmp)
        .status()
        .expect("Failed to run ranlib");
    if !status.success() {
        panic!("ranlib libc_merged_tmp.a failed");
    }

    // アトミックに libc.a を置き換え
    fs::rename(&libc_tmp, &libc_a).expect("Failed to rename libc_merged_tmp.a to libc.a");

    // libg.a (デバッグ版) も同期させる
    let _ = fs::copy(&libc_a, &libg_a);

    fs::remove_dir_all(&merge_dir).unwrap();
    println!("Successfully merged user glue into libc.a");
}
