//! Linux/POSIX 互換スタブ
//!
//! Rust std (build-std) がリンク時に要求する C ライブラリ関数を実装する。
//! 各関数は最小限の実装か、成功を返すスタブ。

use crate::sys::{syscall1, syscall2, syscall3, syscall6, SyscallNumber};

// ─────────────────────────────────────────────────────────────
// errno
// ─────────────────────────────────────────────────────────────

static mut ERRNO_VAL: i32 = 0;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __errno_location() -> *mut i32 {
    &raw mut ERRNO_VAL
}

// ─────────────────────────────────────────────────────────────
// メモリ管理
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mmap(
    addr: *mut u8,
    len: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    offset: i64,
) -> *mut u8 {
    let ret = syscall6(
        SyscallNumber::Mmap as u64,
        addr as u64,
        len as u64,
        prot as u64,
        flags as u64,
        fd as u64,
        offset as u64,
    );
    if ret as i64 == -1 || (ret as i64) < 0 {
        usize::MAX as *mut u8 // MAP_FAILED = (void*)-1
    } else {
        ret as *mut u8
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn munmap(addr: *mut u8, len: usize) -> i32 {
    let ret = syscall2(SyscallNumber::Munmap as u64, addr as u64, len as u64);
    if (ret as i64) < 0 { -1 } else { 0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mprotect(_addr: *mut u8, _len: usize, _prot: i32) -> i32 {
    0 // 成功
}

// ─────────────────────────────────────────────────────────────
// C ライブラリ syscall ラッパー
// ─────────────────────────────────────────────────────────────

/// C の syscall(nr, arg0, arg1, arg2, arg3, arg4, arg5) の実装
/// SysV ABI: nr=rdi, arg0=rsi, arg1=rdx, arg2=rcx, arg3=r8, arg4=r9
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn syscall() {
    core::arch::naked_asm!(
        "mov rax, rdi",    // syscall number
        "mov rdi, rsi",    // arg0
        "mov rsi, rdx",    // arg1
        "mov rdx, rcx",    // arg2
        "mov r10, r8",     // arg3 (Linux: r10, not rcx)
        "mov r8,  r9",     // arg4
        // arg5 would be at [rsp+8] but ignore for now
        "syscall",
        "ret",
    );
}

// ─────────────────────────────────────────────────────────────
// pthread 互換スタブ
// ─────────────────────────────────────────────────────────────

// 単純なスレッドローカルストレージ (シングルスレッド用)
const MAX_TLS_KEYS: usize = 128;
static mut TLS_VALUES: [*mut u8; MAX_TLS_KEYS] = [core::ptr::null_mut(); MAX_TLS_KEYS];
static mut TLS_DESTRUCTORS: [Option<unsafe extern "C" fn(*mut u8)>; MAX_TLS_KEYS] =
    [None; MAX_TLS_KEYS];
static mut TLS_NEXT_KEY: usize = 1; // 0 は無効なキー

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_self() -> u64 {
    1 // スレッド ID = 1 (シングルスレッド)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_key_create(
    key_out: *mut u32,
    destructor: Option<unsafe extern "C" fn(*mut u8)>,
) -> i32 {
    if TLS_NEXT_KEY >= MAX_TLS_KEYS {
        return 12; // ENOMEM
    }
    let key = TLS_NEXT_KEY as u32;
    TLS_NEXT_KEY += 1;
    TLS_DESTRUCTORS[key as usize] = destructor;
    *key_out = key;
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_key_delete(key: u32) -> i32 {
    if (key as usize) < MAX_TLS_KEYS {
        TLS_VALUES[key as usize] = core::ptr::null_mut();
        TLS_DESTRUCTORS[key as usize] = None;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_getspecific(key: u32) -> *mut u8 {
    if (key as usize) < MAX_TLS_KEYS {
        TLS_VALUES[key as usize]
    } else {
        core::ptr::null_mut()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_setspecific(key: u32, val: *const u8) -> i32 {
    if (key as usize) < MAX_TLS_KEYS {
        TLS_VALUES[key as usize] = val as *mut u8;
        0
    } else {
        22 // EINVAL
    }
}

/// pthread_attr_t の最小実装 (128 バイトのダミー構造)
#[repr(C)]
pub struct PthreadAttr {
    _data: [u8; 64],
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_attr_init(attr: *mut PthreadAttr) -> i32 {
    if !attr.is_null() {
        core::ptr::write_bytes(attr as *mut u8, 0, core::mem::size_of::<PthreadAttr>());
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_attr_destroy(_attr: *mut PthreadAttr) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_attr_getguardsize(
    _attr: *const PthreadAttr,
    size_out: *mut usize,
) -> i32 {
    if !size_out.is_null() {
        *size_out = 4096;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_attr_getstack(
    _attr: *const PthreadAttr,
    stack_addr_out: *mut *mut u8,
    stack_size_out: *mut usize,
) -> i32 {
    if !stack_addr_out.is_null() {
        *stack_addr_out = core::ptr::null_mut();
    }
    if !stack_size_out.is_null() {
        *stack_size_out = 0;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_getattr_np(
    _thread: u64,
    attr: *mut PthreadAttr,
) -> i32 {
    pthread_attr_init(attr)
}

// ─────────────────────────────────────────────────────────────
// シグナル
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sigaction(
    _signum: i32,
    _act: *const u8,
    _oldact: *mut u8,
) -> i32 {
    0 // 成功
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sigaltstack(_ss: *const u8, _oss: *mut u8) -> i32 {
    0
}

// ─────────────────────────────────────────────────────────────
// 時間・待機
// ─────────────────────────────────────────────────────────────

/// nanosleep(req, rem) - 簡易実装 (yield で代用)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nanosleep(_req: *const u8, _rem: *mut u8) -> i32 {
    // 少しだけ yield
    for _ in 0..10 {
        syscall1(SyscallNumber::Yield as u64, 0);
    }
    0
}

/// pause() - シグナル待ち (実装なし: すぐリターン)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pause() -> i32 {
    -1 // EINTR
}

// ─────────────────────────────────────────────────────────────
// I/O
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fcntl(_fd: i32, _cmd: i32, _arg: i64) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pipe2(_pipefd: *mut i32, _flags: i32) -> i32 {
    -1 // ENOSYS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn recv(
    _fd: i32,
    _buf: *mut u8,
    _len: usize,
    _flags: i32,
) -> isize {
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn socketpair(
    _domain: i32,
    _type_: i32,
    _protocol: i32,
    _sv: *mut i32,
) -> i32 {
    -1 // ENOSYS
}

// ─────────────────────────────────────────────────────────────
// Linux AUX ベクタ
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn getauxval(_type_: u64) -> u64 {
    0
}

// ─────────────────────────────────────────────────────────────
// プロセス管理スタブ
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waitpid(_pid: i32, _status: *mut i32, _options: i32) -> i32 {
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dup(_oldfd: i32) -> i32 {
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dup2(_oldfd: i32, _newfd: i32) -> i32 {
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn chdir(_path: *const u8) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn chroot(_path: *const u8) -> i32 {
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn setuid(_uid: u32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn setgid(_gid: u32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn setgroups(_size: usize, _list: *const u32) -> i32 {
    0
}

// ─────────────────────────────────────────────────────────────
// posix_spawn スタブ
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_spawn_file_actions_init(_actions: *mut u8) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_spawn_file_actions_adddup2(
    _actions: *mut u8, _fd: i32, _newfd: i32,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_spawnattr_init(_attr: *mut u8) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_spawnattr_setpgroup(_attr: *mut u8, _pgroup: i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_spawnattr_setsigdefault(
    _attr: *mut u8, _sigset: *const u8,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_spawnattr_setflags(_attr: *mut u8, _flags: i16) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_spawnp(
    _pid_out: *mut i32,
    _file: *const u8,
    _file_actions: *const u8,
    _attr: *const u8,
    _argv: *const *const u8,
    _envp: *const *const u8,
) -> i32 {
    -1 // ENOSYS
}

// ─────────────────────────────────────────────────────────────
// シグナルセット
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sigemptyset(set: *mut u8) -> i32 {
    if !set.is_null() {
        core::ptr::write_bytes(set, 0, 128); // sigset_t は最大 128 バイト
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sigaddset(_set: *mut u8, _signum: i32) -> i32 {
    0
}

// ─────────────────────────────────────────────────────────────
// ソケット追加スタブ
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn recvmsg(_fd: i32, _msg: *mut u8, _flags: i32) -> isize {
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sendmsg(_fd: i32, _msg: *const u8, _flags: i32) -> isize {
    -1
}

// ─────────────────────────────────────────────────────────────
// システム設定
// ─────────────────────────────────────────────────────────────

/// sysconf - システム設定値を取得
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sysconf(name: i32) -> i64 {
    const SC_PAGESIZE: i32 = 30;
    const SC_NPROCESSORS_ONLN: i32 = 84;
    const SC_NPROCESSORS_CONF: i32 = 83;
    const SC_GETPW_R_SIZE_MAX: i32 = 70;
    const SC_GETGR_R_SIZE_MAX: i32 = 69;
    const SC_OPEN_MAX: i32 = 4;
    const SC_CLK_TCK: i32 = 2;

    match name {
        SC_PAGESIZE => 4096,
        SC_NPROCESSORS_ONLN | SC_NPROCESSORS_CONF => 1,
        SC_GETPW_R_SIZE_MAX | SC_GETGR_R_SIZE_MAX => 1024,
        SC_OPEN_MAX => 256,
        SC_CLK_TCK => 100,
        _ => -1,
    }
}

// ─────────────────────────────────────────────────────────────
// プロセス制御追加スタブ
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn setpgid(_pid: i32, _pgid: i32) -> i32 { 0 }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn setsid() -> i32 { 1 }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn execvp(_file: *const u8, _argv: *const *const u8) -> i32 { -1 }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn waitid(_which: i32, _id: u32, _infop: *mut u8, _options: i32) -> i32 { -1 }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn poll(_fds: *mut u8, _nfds: u64, _timeout: i32) -> i32 { 0 }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ioctl(_fd: i32, _request: u64, _arg: u64) -> i32 { -1 }

// ─────────────────────────────────────────────────────────────
// posix_spawn 追加スタブ
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_spawn_file_actions_destroy(_actions: *mut u8) -> i32 { 0 }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_spawn_file_actions_addchdir_np(
    _actions: *mut u8, _path: *const u8,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_spawnattr_destroy(_attr: *mut u8) -> i32 { 0 }

// ─────────────────────────────────────────────────────────────
// メモリ
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_memalign(
    memptr: *mut *mut u8, alignment: usize, size: usize,
) -> i32 {
    extern "C" { fn malloc(size: usize) -> *mut u8; }
    let ptr = malloc(size + alignment);
    if ptr.is_null() { return 12; } // ENOMEM
    let addr = ptr as usize;
    let aligned = (addr + alignment - 1) & !(alignment - 1);
    *memptr = aligned as *mut u8;
    0
}

// ─────────────────────────────────────────────────────────────
// 時刻 (clock_gettime の C ラッパー)
// ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn clock_gettime(clk_id: i32, tp: *mut u8) -> i32 {
    // timespec: { tv_sec: i64, tv_nsec: i64 }
    // タイマーティック (1ティック = 1ms) から計算
    let ticks = syscall1(SyscallNumber::GetTicks as u64, 0);
    let sec = (ticks / 1000) as i64;
    let nsec = ((ticks % 1000) * 1_000_000) as i64;
    if !tp.is_null() {
        core::ptr::write(tp as *mut i64, sec);
        core::ptr::write((tp as *mut i64).add(1), nsec);
    }
    0
}
