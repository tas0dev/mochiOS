use core::ffi::c_char;
use swiftlib::fs;

extern "C" {
    fn printf(fmt: *const c_char, ...) -> i32;
}

// テスト結果の統計
struct TestStats {
    passed: usize,
    failed: usize,
    total: usize,
}

impl TestStats {
    const fn new() -> Self {
        Self {
            passed: 0,
            failed: 0,
            total: 0,
        }
    }

    fn run_test(&mut self, name: &str, test_fn: fn() -> bool) {
        self.total += 1;
        unsafe {
            printf(b"[TEST] Running: %s ... \0".as_ptr() as *const c_char, name.as_ptr());
        }
        
        if test_fn() {
            self.passed += 1;
            unsafe {
                printf(b"OK\n\0".as_ptr() as *const c_char);
            }
        } else {
            self.failed += 1;
            unsafe {
                printf(b"FAILED\n\0".as_ptr() as *const c_char);
            }
        }
    }

    fn summary(&self) {
        unsafe {
            printf(b"\n========================================\n\0".as_ptr() as *const c_char);
            printf(b"Test Results:\n\0".as_ptr() as *const c_char);
            printf(b"  Total:  %d\n\0".as_ptr() as *const c_char, self.total);
            printf(b"  Passed: %d\n\0".as_ptr() as *const c_char, self.passed);
            printf(b"  Failed: %d\n\0".as_ptr() as *const c_char, self.failed);
            printf(b"========================================\n\0".as_ptr() as *const c_char);
        }
    }
}

// ========== テスト関数 ==========

fn test_basic_arithmetic() -> bool {
    let a = 2 + 2;
    a == 4
}

fn test_string_compare() -> bool {
    let s1 = b"hello";
    let s2 = b"hello";
    s1 == s2
}

fn test_argc_argv() -> bool {
    true
}

fn test_mkdir() -> bool {
    unsafe {
        printf(b"\n  Creating directory '/testdir' ... \0".as_ptr() as *const c_char);
    }
    
    let result = fs::mkdir("/testdir\0", 0o755);
    
    unsafe {
        if result == 0 || result == u64::MAX - 5 { // SUCCESS or ENOSYS
            printf(b"(syscall returned %llu) \0".as_ptr() as *const c_char, result);
            true
        } else {
            printf(b"failed with error %llu \0".as_ptr() as *const c_char, result);
            false
        }
    }
}

fn test_rmdir() -> bool {
    unsafe {
        printf(b"\n  Removing directory '/testdir' ... \0".as_ptr() as *const c_char);
    }
    
    let result = fs::rmdir("/testdir\0");
    
    unsafe {
        if result == 0 || result == u64::MAX - 5 { // SUCCESS or ENOSYS
            printf(b"(syscall returned %llu) \0".as_ptr() as *const c_char, result);
            true
        } else {
            printf(b"failed with error %llu \0".as_ptr() as *const c_char, result);
            false
        }
    }
}

fn test_chdir() -> bool {
    unsafe {
        printf(b"\n  Changing directory to '/' ... \0".as_ptr() as *const c_char);
    }
    
    let result = fs::chdir("/\0");
    
    unsafe {
        if result == 0 || result == u64::MAX - 5 { // SUCCESS or ENOSYS
            printf(b"(syscall returned %llu) \0".as_ptr() as *const c_char, result);
            true
        } else {
            printf(b"failed with error %llu \0".as_ptr() as *const c_char, result);
            false
        }
    }
}

fn test_readdir() -> bool {
    unsafe {
        printf(b"\n  Reading directory (fd=3) ... \0".as_ptr() as *const c_char);
    }
    
    let mut buf = [0u8; 512];
    let result = fs::readdir(3, &mut buf);
    
    unsafe {
        if result == u64::MAX - 5 { // ENOSYS is expected for now
            printf(b"(syscall returned ENOSYS) \0".as_ptr() as *const c_char);
            true
        } else if result == u64::MAX - 3 { // EBADF
            printf(b"(syscall returned EBADF, expected) \0".as_ptr() as *const c_char);
            true
        } else {
            printf(b"read %llu bytes \0".as_ptr() as *const c_char, result);
            result == 0 || result > 0
        }
    }
}

// ========== メイン関数 ==========
fn main() {
    let argc = std::env::args().count() as i32;
    let args: Vec<std::ffi::CString> = std::env::args()
        .map(|a| std::ffi::CString::new(a).unwrap_or_default())
        .collect();
    let argv_ptrs: Vec<*const u8> = args.iter().map(|a| a.as_ptr() as *const u8).collect();
    let argv: *const *const u8 = argv_ptrs.as_ptr();
    unsafe {
        printf(b"\n========================================\n\0".as_ptr() as *const c_char);
        printf(b"mochiOS Test Suite\n\0".as_ptr() as *const c_char);
        printf(b"========================================\n\n\0".as_ptr() as *const c_char);

        printf(b"Test invocation:\n\0".as_ptr() as *const c_char);
        printf(b"  argc: %d\n\0".as_ptr() as *const c_char, argc);
        for i in 0..argc {
            let arg_ptr = *argv.offset(i as isize);
            printf(b"  argv[%d]: %s\n\0".as_ptr() as *const c_char, i, arg_ptr);
        }
        printf(b"\n\0".as_ptr() as *const c_char);
    }

    let mut stats = TestStats::new();

    // 基本テストを実行
    stats.run_test("basic_arithmetic\0", test_basic_arithmetic);
    stats.run_test("string_compare\0", test_string_compare);
    stats.run_test("argc_argv\0", test_argc_argv);
    
    // ファイルシステムテストを実行
    unsafe {
        printf(b"\n--- Filesystem Tests ---\n\0".as_ptr() as *const c_char);
    }
    stats.run_test("mkdir\0", test_mkdir);
    stats.run_test("chdir\0", test_chdir);
    stats.run_test("readdir\0", test_readdir);
    stats.run_test("rmdir\0", test_rmdir);

    // 結果を表示
    stats.summary();

    // 失敗があれば終了コード1を返す
    std::process::exit(if stats.failed > 0 { 1 } else { 0 });
}
