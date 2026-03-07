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
        let full = std::path::Path::new(dir).join(name);
        Command::new(&full).arg("--version").output().is_ok()
    })
}

/// Return the full path to a tool, checking well-known locations before name-only fallback.
fn find_tool(name: &str) -> String {
    if Command::new(name).arg("--version").output().is_ok() {
        return name.to_string();
    }
    for dir in EXTRA_TOOL_PATHS {
        let full = std::path::Path::new(dir).join(name);
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
    println!("Building user libs...");

    if !libc_dir.exists() {
        fs::create_dir_all(libc_dir).expect("Failed to create libc dir");
    }

    let crt_src = user_dir.join("crt.rs");
    let crt_obj = libc_dir.join("crt0.o");

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
    let lib_src = user_dir.join("lib.rs");
    let glue_lib = libc_dir.join("libuserglue.a");

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

    // 3. libc.a にマージ
    let merge_dir = libc_dir.join("merge_tmp");
    if merge_dir.exists() {
        fs::remove_dir_all(&merge_dir).unwrap();
    }
    fs::create_dir(&merge_dir).unwrap();

    let libc_a = libc_dir.join("libc.a");
    let libglue_a = glue_lib;

    let status = Command::new("ar")
        .current_dir(&merge_dir)
        .arg("x")
        .arg(&libc_a)
        .status()
        .expect("Failed to extract libc.a");
    if !status.success() {
        panic!("ar x libc.a failed");
    }

    let status = Command::new("ar")
        .current_dir(&merge_dir)
        .arg("x")
        .arg(&libglue_a)
        .status()
        .expect("Failed to extract libuserglue.a");
    if !status.success() {
        panic!("ar x libuserglue.a failed");
    }

    let status = Command::new("sh")
        .current_dir(&merge_dir)
        .arg("-c")
        .arg(format!("ar rcs {} *.o", libc_a.to_str().unwrap()))
        .status()
        .expect("Failed to repack libc.a");

    if !status.success() {
        panic!("ar rcs libc.a failed");
    }

    fs::remove_dir_all(&merge_dir).unwrap();
    println!("Successfully merged user glue into libc.a");
}
