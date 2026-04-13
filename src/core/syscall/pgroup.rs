//! プロセスグループ・セッション関連のシステムコール

use super::types::{EINVAL, EPERM, ESRCH, SUCCESS};

#[inline]
fn current_pid() -> Option<crate::task::ids::ProcessId> {
    crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
}

/// Getppid システムコール
pub fn getppid() -> u64 {
    let pid = match current_pid() {
        Some(p) => p,
        None => return 0,
    };
    crate::task::with_process(pid, |p| {
        p.parent_id().map(|ppid| ppid.as_u64()).unwrap_or(1)
    })
    .unwrap_or(1)
}

/// Getpgid システムコール
///
/// pid=0 の場合は呼び出しプロセス自身のグループ ID を返す。
pub fn getpgid(pid_arg: u64) -> u64 {
    let target_pid = if pid_arg == 0 {
        match current_pid() {
            Some(p) => p,
            None => return ESRCH,
        }
    } else {
        crate::task::ids::ProcessId::from_u64(pid_arg)
    };

    crate::task::with_process(target_pid, |p| p.pgid()).unwrap_or_else(|| ESRCH)
}

/// Setpgid システムコール
///
/// pid=0 は自プロセス、pgid=0 はプロセス自身の PID を使用する。
pub fn setpgid(pid_arg: u64, pgid_arg: u64) -> u64 {
    let caller = match current_pid() {
        Some(p) => p,
        None => return ESRCH,
    };
    let target_pid = if pid_arg == 0 {
        caller
    } else {
        crate::task::ids::ProcessId::from_u64(pid_arg)
    };

    // 呼び出し元は自分自身または直接の子プロセスのみ変更可能
    let is_child = if target_pid != caller {
        crate::task::with_process(target_pid, |p| p.parent_id() == Some(caller)).unwrap_or(false)
    } else {
        true
    };
    if !is_child {
        return EPERM;
    }

    let new_pgid = if pgid_arg == 0 {
        target_pid.as_u64()
    } else {
        pgid_arg
    };

    match crate::task::with_process_mut(target_pid, |p| {
        p.set_pgid(new_pgid);
    }) {
        Some(()) => SUCCESS,
        None => ESRCH,
    }
}

/// Setsid システムコール
///
/// 新しいセッションを作成し、呼び出しプロセスがそのリーダーになる。
/// sid = pgid = pid に設定する。
pub fn setsid() -> u64 {
    let pid = match current_pid() {
        Some(p) => p,
        None => return ESRCH,
    };
    let pid_val = pid.as_u64();
    crate::task::with_process_mut(pid, |p| {
        p.set_pgid(pid_val);
        p.set_sid(pid_val);
        pid_val
    })
    .unwrap_or_else(|| ESRCH)
}

/// Getsid システムコール
pub fn getsid(pid_arg: u64) -> u64 {
    let target_pid = if pid_arg == 0 {
        match current_pid() {
            Some(p) => p,
            None => return ESRCH,
        }
    } else {
        crate::task::ids::ProcessId::from_u64(pid_arg)
    };

    crate::task::with_process(target_pid, |p| p.sid()).unwrap_or_else(|| ESRCH)
}

/// ioctl システムコール
///
/// 対応コマンド:
/// - TIOCGPGRP (0x540f): フォアグラウンドプロセスグループを取得
/// - TIOCSPGRP (0x5410): フォアグラウンドプロセスグループを設定
/// - TIOCGWINSZ (0x5413): ウィンドウサイズ取得
/// - TCGETS (0x5401): termios 取得
/// - TCSETS/TCSETSW/TCSETSF (0x5402-0x5404): termios 設定
pub fn ioctl(fd: u64, request: u64, arg: u64) -> u64 {
    const TIOCGPGRP: u64 = 0x540f;
    const TIOCSPGRP: u64 = 0x5410;
    const TIOCGWINSZ: u64 = 0x5413;
    const TCGETS: u64 = 0x5401;
    const TCSETS: u64 = 0x5402;
    const TCSETSW: u64 = 0x5403;
    const TCSETSF: u64 = 0x5404;
    const TIOCSWINSZ: u64 = 0x5414;

    match request {
        TIOCGPGRP => {
            if arg == 0 || !crate::syscall::validate_user_ptr(arg, 4) {
                return EINVAL;
            }
            let pgid = match current_pid() {
                Some(pid) => crate::task::with_process(pid, |p| p.pgid()).unwrap_or(1),
                None => return EINVAL,
            };
            crate::syscall::with_user_memory_access(|| unsafe {
                core::ptr::write_unaligned(arg as *mut u32, pgid as u32);
            });
            SUCCESS
        }
        TIOCSPGRP => SUCCESS,
        TIOCGWINSZ => {
            // struct winsize: { ws_row(u16), ws_col(u16), ws_xpixel(u16), ws_ypixel(u16) }
            if arg == 0 || !crate::syscall::validate_user_ptr(arg, 8) {
                return EINVAL;
            }
            crate::syscall::with_user_memory_access(|| unsafe {
                let buf = core::slice::from_raw_parts_mut(arg as *mut u8, 8);
                buf.fill(0);
                buf[0..2].copy_from_slice(&24u16.to_ne_bytes()); // ws_row
                buf[2..4].copy_from_slice(&80u16.to_ne_bytes()); // ws_col
            });
            SUCCESS
        }
        TIOCSWINSZ => SUCCESS,
        TCGETS => {
            // 最小互換: カーネル termios 相当の先頭 36 バイトのみを書き込む。
            // ここを過大に書くと、呼び出し側のスタック上バッファを破壊し得る。
            // レイアウト: c_iflag/c_oflag/c_cflag/c_lflag(各4) + c_line(1) + c_cc[19](19) = 36
            const TERMIOS_SIZE: u64 = 36;
            if arg == 0 || !crate::syscall::validate_user_ptr(arg, TERMIOS_SIZE) {
                return EINVAL;
            }
            crate::syscall::with_user_memory_access(|| unsafe {
                let buf = core::slice::from_raw_parts_mut(arg as *mut u8, TERMIOS_SIZE as usize);
                buf.fill(0);
                // c_cflag: CS8(0x30) | CREAD(0x80) | CLOCAL(0x800)
                let cflag: u32 = 0x30 | 0x80 | 0x800;
                buf[8..12].copy_from_slice(&cflag.to_ne_bytes());
                // c_cc[VMIN]=1, c_cc[VTIME]=0
                buf[17] = 1;
            });
            SUCCESS
        }
        TCSETS | TCSETSW | TCSETSF => SUCCESS, // termios 設定は無視して成功
        _ => EINVAL,
    }
}

/// access システムコール（ファイルアクセス可能性チェック）
///
/// initfs/rootfs にファイルが存在すれば常に成功を返す。
pub fn access(path_ptr: u64, _mode: u64) -> u64 {
    use super::types::ENOENT;
    if path_ptr == 0 {
        return EINVAL;
    }
    let path = match crate::syscall::read_user_cstring(path_ptr, 1024) {
        Ok(s) => s,
        Err(e) => return e,
    };
    if crate::init::fs::file_metadata(&path).is_some() {
        SUCCESS
    } else {
        ENOENT
    }
}

/// getuid / geteuid / getgid / getegid システムコール（常に 0 = root を返す）
pub fn getuid() -> u64 {
    0
}
pub fn getgid() -> u64 {
    0
}
pub fn geteuid() -> u64 {
    0
}
pub fn getegid() -> u64 {
    0
}

/// uname システムコール
///
/// struct utsname のレイアウト (Linux x86_64): 各フィールド 65 バイト × 6 = 390 バイト
/// sysname, nodename, release, version, machine, domainname
pub fn uname(buf_ptr: u64) -> u64 {
    const FIELD_LEN: usize = 65;
    const UTSNAME_SIZE: u64 = (FIELD_LEN * 6) as u64;
    if buf_ptr == 0 || !crate::syscall::validate_user_ptr(buf_ptr, UTSNAME_SIZE) {
        return EINVAL;
    }
    let fields: [&[u8]; 6] = [
        b"mochiOS",       // sysname
        b"mochi",         // nodename
        b"0.1.0",         // release
        b"mochiOS 0.1.0", // version
        b"x86_64",        // machine
        b"",              // domainname
    ];
    crate::syscall::with_user_memory_access(|| unsafe {
        let buf = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, UTSNAME_SIZE as usize);
        buf.fill(0);
        for (i, f) in fields.iter().enumerate() {
            let off = i * FIELD_LEN;
            let n = f.len().min(FIELD_LEN - 1);
            buf[off..off + n].copy_from_slice(&f[..n]);
        }
    });
    SUCCESS
}

/// nanosleep システムコール
///
/// struct timespec { tv_sec: i64, tv_nsec: i64 } を受け取りスリープする。
pub fn nanosleep(req_ptr: u64, _rem_ptr: u64) -> u64 {
    if req_ptr == 0 || !crate::syscall::validate_user_ptr(req_ptr, 16) {
        return EINVAL;
    }
    let (secs, nsecs) = crate::syscall::with_user_memory_access(|| unsafe {
        let secs = core::ptr::read_unaligned(req_ptr as *const i64);
        let nsecs = core::ptr::read_unaligned((req_ptr + 8) as *const i64);
        (secs, nsecs)
    });
    if secs < 0 || nsecs < 0 || nsecs >= 1_000_000_000 {
        return EINVAL;
    }
    let total_ms = (secs as u64) * 1000 + (nsecs as u64) / 1_000_000;
    if total_ms > 0 {
        crate::syscall::process::sleep(total_ms);
    }
    SUCCESS
}

/// mprotect システムコール（スタブ）
///
/// addr=0（nullページ）への保護変更は EINVAL を返す。
/// それ以外は SUCCESS を返す（未実装）。
pub fn mprotect(addr: u64, _len: u64, _prot: u64) -> u64 {
    if addr < 0x1000 {
        return super::types::EINVAL;
    }
    SUCCESS
}

/// getrlimit システムコール（リソース上限を無限大で返す）
pub fn getrlimit(_resource: u64, rlim_ptr: u64) -> u64 {
    if rlim_ptr == 0 || !crate::syscall::validate_user_ptr(rlim_ptr, 16) {
        return EINVAL;
    }
    // struct rlimit { rlim_cur: u64, rlim_max: u64 }
    crate::syscall::with_user_memory_access(|| unsafe {
        core::ptr::write_unaligned(rlim_ptr as *mut u64, u64::MAX);
        core::ptr::write_unaligned((rlim_ptr + 8) as *mut u64, u64::MAX);
    });
    SUCCESS
}

/// prlimit64 システムコール（スタブ: 無限大を返し、設定を無視）
pub fn prlimit64(_pid: u64, _resource: u64, _new_limit: u64, old_limit: u64) -> u64 {
    if old_limit != 0 {
        return getrlimit(0, old_limit);
    }
    SUCCESS
}

/// set_tid_address システムコール
///
/// musl libc の初期化で呼ばれる。現在のスレッド ID を返す。
pub fn set_tid_address(_tidptr: u64) -> u64 {
    match crate::task::current_thread_id() {
        Some(tid) => tid.as_u64(),
        None => 1,
    }
}
