//! プロセスグループ・セッション関連のシステムコール

use super::types::{EFAULT, EINVAL, EPERM, ESRCH, SUCCESS};
use crate::task::fd_table::FD_BASE;

const POLLIN: u16 = 0x0001;
const POLLOUT: u16 = 0x0004;
const POLLRDNORM: u16 = 0x0040;
const POLLWRNORM: u16 = 0x0100;

fn stdin_ready() -> bool {
    crate::syscall::tty::has_pending_input()
}

fn is_tty_fd(fd: i32) -> bool {
    if fd < 0 {
        return false;
    }
    if fd <= 2 {
        return true;
    }
    if (fd as u64) < FD_BASE as u64 {
        return false;
    }
    let pid = match current_pid() {
        Some(p) => p,
        None => return false,
    };
    crate::task::with_process(pid, |p| {
        p.fd_table()
            .get(fd as usize)
            .is_some_and(|fh| fh.dir_path.as_deref() == Some("/dev/tty"))
    })
    .unwrap_or(false)
}

fn stdin_ready_for_fd(fd: i32) -> bool {
    is_tty_fd(fd) && stdin_ready()
}

fn stdout_ready(fd: i32) -> bool {
    is_tty_fd(fd)
}

fn wait_until_ready_or_timeout(mut timeout_ms: i64, mut ready_fn: impl FnMut() -> bool) -> bool {
    if ready_fn() {
        return true;
    }
    if timeout_ms == 0 {
        return false;
    }
    if timeout_ms < 0 {
        loop {
            if ready_fn() {
                return true;
            }
            crate::task::yield_now();
        }
    }
    while timeout_ms > 0 {
        crate::syscall::process::sleep(1);
        if ready_fn() {
            return true;
        }
        timeout_ms -= 1;
    }
    false
}

fn fdset_len_bytes(nfds: u64) -> Option<u64> {
    let words = nfds.checked_add(63)?.checked_div(64)?;
    words.checked_mul(8)
}

fn fdset_test(ptr: u64, fd: u64) -> bool {
    let word_off = (fd / 64) * 8;
    let bit = (fd % 64) as u32;
    crate::syscall::with_user_memory_access(|| unsafe {
        let w = core::ptr::read_unaligned((ptr + word_off) as *const u64);
        (w & (1u64 << bit)) != 0
    })
}

fn fdset_clear_all(ptr: u64, len: u64) {
    crate::syscall::with_user_memory_access(|| unsafe {
        core::ptr::write_bytes(ptr as *mut u8, 0, len as usize);
    });
}

fn fdset_set(ptr: u64, fd: u64) {
    let word_off = (fd / 64) * 8;
    let bit = (fd % 64) as u32;
    crate::syscall::with_user_memory_access(|| unsafe {
        let p = (ptr + word_off) as *mut u64;
        let v = core::ptr::read_unaligned(p as *const u64);
        core::ptr::write_unaligned(p, v | (1u64 << bit));
    });
}

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
    const FIONREAD: u64 = 0x541b;

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
            crate::syscall::tty::get_winsize(arg)
        }
        TIOCSWINSZ => crate::syscall::tty::set_winsize(arg),
        TCGETS => crate::syscall::tty::tcgets(arg),
        TCSETS | TCSETSW | TCSETSF => crate::syscall::tty::tcsets(arg),
        FIONREAD => {
            if arg == 0 || !crate::syscall::validate_user_ptr(arg, 4) {
                return EINVAL;
            }
            let n = crate::syscall::tty::pending_input_len() as u32;
            crate::syscall::with_user_memory_access(|| unsafe {
                core::ptr::write_unaligned(arg as *mut u32, n);
            });
            SUCCESS
        }
        _ => EINVAL,
    }
}

/// poll システムコール（最小実装）
///
/// TTY fd の read/write readiness を返す。
pub fn poll(fds_ptr: u64, nfds: u64, timeout_arg: u64) -> u64 {
    const POLLFD_SIZE: u64 = 8; // i32 fd, i16 events, i16 revents
    if nfds == 0 {
        return 0;
    }
    let total = match nfds.checked_mul(POLLFD_SIZE) {
        Some(v) => v,
        None => return EINVAL,
    };
    if fds_ptr == 0 || !crate::syscall::validate_user_ptr(fds_ptr, total) {
        return EFAULT;
    }
    let timeout_ms = i64::from_ne_bytes(timeout_arg.to_ne_bytes());

    let mut eval_ready = || -> u64 {
        let mut ready_count = 0u64;
        for i in 0..nfds {
            let base = fds_ptr + i * POLLFD_SIZE;
            let (fd, events) = crate::syscall::with_user_memory_access(|| unsafe {
                let fd = core::ptr::read_unaligned(base as *const i32);
                let events = core::ptr::read_unaligned((base + 4) as *const u16);
                (fd, events)
            });
            let mut revents: u16 = 0;
            if fd >= 0 {
                if (events & (POLLIN | POLLRDNORM)) != 0 && stdin_ready_for_fd(fd) {
                    revents |= POLLIN;
                }
                if (events & (POLLOUT | POLLWRNORM)) != 0 && stdout_ready(fd) {
                    revents |= POLLOUT;
                }
            }
            crate::syscall::with_user_memory_access(|| unsafe {
                core::ptr::write_unaligned((base + 6) as *mut u16, revents);
            });
            if revents != 0 {
                ready_count += 1;
            }
        }
        ready_count
    };

    let initial = eval_ready();
    if initial > 0 {
        return initial;
    }
    let woke = wait_until_ready_or_timeout(timeout_ms, || eval_ready() > 0);
    if !woke {
        return 0;
    }
    eval_ready()
}

/// ppoll システムコール（最小実装）
///
/// timeout は timespec*。sigmask/sigsetsize は現状未使用。
pub fn ppoll(fds_ptr: u64, nfds: u64, timeout_ptr: u64, _sigmask_ptr: u64, _sigsetsize: u64) -> u64 {
    let timeout_ms_u64 = if timeout_ptr == 0 {
        u64::MAX
    } else {
        if !crate::syscall::validate_user_ptr(timeout_ptr, 16) {
            return EFAULT;
        }
        let (sec, nsec) = crate::syscall::with_user_memory_access(|| unsafe {
            let sec = core::ptr::read_unaligned(timeout_ptr as *const i64);
            let nsec = core::ptr::read_unaligned((timeout_ptr + 8) as *const i64);
            (sec, nsec)
        });
        if sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
            return EINVAL;
        }
        (sec as u64)
            .saturating_mul(1000)
            .saturating_add((nsec as u64) / 1_000_000)
    };
    poll(fds_ptr, nfds, timeout_ms_u64)
}

/// pselect6/select システムコール（最小実装）
///
/// readfds/writefds のうち TTY fd の readiness を判定する。
pub fn pselect6(
    nfds: u64,
    readfds_ptr: u64,
    writefds_ptr: u64,
    _exceptfds_ptr: u64,
    timeout_ptr: u64,
    _sigmask_ptr: u64,
) -> u64 {
    let mut timeout_ms = -1i64;
    if timeout_ptr != 0 {
        if !crate::syscall::validate_user_ptr(timeout_ptr, 16) {
            return EFAULT;
        }
        let (sec, nsec) = crate::syscall::with_user_memory_access(|| unsafe {
            let sec = core::ptr::read_unaligned(timeout_ptr as *const i64);
            let nsec = core::ptr::read_unaligned((timeout_ptr + 8) as *const i64);
            (sec, nsec)
        });
        if sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
            return EINVAL;
        }
        timeout_ms = sec.saturating_mul(1000).saturating_add(nsec / 1_000_000);
    }

    let set_len = match fdset_len_bytes(nfds) {
        Some(v) => v,
        None => return EINVAL,
    };
    if readfds_ptr != 0 && !crate::syscall::validate_user_ptr(readfds_ptr, set_len) {
        return EFAULT;
    }
    if writefds_ptr != 0 && !crate::syscall::validate_user_ptr(writefds_ptr, set_len) {
        return EFAULT;
    }

    let read_interest = if readfds_ptr != 0 {
        let mut v: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
        for fd in 0..nfds {
            if fdset_test(readfds_ptr, fd) {
                v.push(fd);
            }
        }
        v
    } else {
        alloc::vec::Vec::new()
    };
    let write_interest = if writefds_ptr != 0 {
        let mut v: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
        for fd in 0..nfds {
            if fdset_test(writefds_ptr, fd) {
                v.push(fd);
            }
        }
        v
    } else {
        alloc::vec::Vec::new()
    };

    let mut eval_ready = || -> u64 {
        let mut ready_count = 0u64;
        if readfds_ptr != 0 {
            let mut ready_fds: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
            for &fd in &read_interest {
                if stdin_ready_for_fd(fd as i32) {
                    ready_fds.push(fd);
                }
            }
            fdset_clear_all(readfds_ptr, set_len);
            for fd in ready_fds {
                fdset_set(readfds_ptr, fd);
                ready_count += 1;
            }
        }
        if writefds_ptr != 0 {
            let mut ready_fds: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
            for &fd in &write_interest {
                if stdout_ready(fd as i32) {
                    ready_fds.push(fd);
                }
            }
            fdset_clear_all(writefds_ptr, set_len);
            for fd in ready_fds {
                fdset_set(writefds_ptr, fd);
                ready_count += 1;
            }
        }
        ready_count
    };

    let initial = eval_ready();
    if initial > 0 {
        return initial;
    }
    let woke = wait_until_ready_or_timeout(timeout_ms, || eval_ready() > 0);
    if !woke {
        if readfds_ptr != 0 {
            fdset_clear_all(readfds_ptr, set_len);
        }
        if writefds_ptr != 0 {
            fdset_clear_all(writefds_ptr, set_len);
        }
        return 0;
    }
    eval_ready()
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
