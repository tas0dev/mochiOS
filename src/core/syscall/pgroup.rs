//! プロセスグループ・セッション関連のシステムコール

use super::types::{EFAULT, EINVAL, ENOMEM, ENOTSUP, EPERM, ESRCH, SUCCESS};
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
        p.fd_table().get(fd as usize).is_some_and(|fh| {
            crate::syscall::fs::is_tty_like_path(fh.dir_path.as_deref().unwrap_or(""))
        })
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
    let addr = match ptr.checked_add(word_off) {
        Some(addr) => addr,
        None => return false,
    };
    crate::syscall::read_user_u64(addr)
        .map(|w| (w & (1u64 << bit)) != 0)
        .unwrap_or(false)
}

fn fdset_clear_all(ptr: u64, len: u64) -> Result<(), u64> {
    let zero = [0u8; 64];
    let mut written = 0u64;
    while written < len {
        let chunk = core::cmp::min((len - written) as usize, zero.len());
        crate::syscall::copy_to_user(ptr + written, &zero[..chunk])?;
        written += chunk as u64;
    }
    Ok(())
}

fn fdset_set(ptr: u64, fd: u64) -> Result<(), u64> {
    let word_off = (fd / 64) * 8;
    let bit = (fd % 64) as u32;
    let addr = ptr.checked_add(word_off).ok_or(EFAULT)?;
    let v = crate::syscall::read_user_u64(addr)?;
    crate::syscall::write_user_u64(addr, v | (1u64 << bit))
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
    const TCGETA: u64 = 0x5405;
    const TCSETA: u64 = 0x5406;
    const TCSETAW: u64 = 0x5407;
    const TCSETAF: u64 = 0x5408;
    const TCGETS2: u64 = 0x802C_542A;
    const TCSETS2: u64 = 0x402C_542B;
    const TCSETSW2: u64 = 0x402C_542C;
    const TCSETSF2: u64 = 0x402C_542D;
    // newlib(sysvi386) の termios が使う ioctl 番号
    const XCGETA: u64 = ((b'x' as u64) << 8) | 1;
    const XCSETA: u64 = ((b'x' as u64) << 8) | 2;
    const XCSETAW: u64 = ((b'x' as u64) << 8) | 3;
    const XCSETAF: u64 = ((b'x' as u64) << 8) | 4;
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
            crate::syscall::write_user_u32(arg, pgid as u32)
                .map(|_| SUCCESS)
                .unwrap_or_else(|e| e)
        }
        TIOCSPGRP => ENOTSUP,
        TIOCGWINSZ => crate::syscall::tty::get_winsize(arg),
        TIOCSWINSZ => ENOTSUP,
        TCGETS | TCGETS2 => crate::syscall::tty::tcgets(arg),
        XCGETA => crate::syscall::tty::tcgeta(arg),
        TCGETA => crate::syscall::tty::tcgeta(arg),
        TCSETS | TCSETSW | TCSETSF | TCSETS2 | TCSETSW2 | TCSETSF2 => ENOTSUP,
        XCSETA | XCSETAW | XCSETAF => ENOTSUP,
        TCSETA | TCSETAW | TCSETAF => ENOTSUP,
        FIONREAD => {
            if arg == 0 || !crate::syscall::validate_user_ptr(arg, 4) {
                return EINVAL;
            }
            let n = crate::syscall::tty::pending_input_len() as u32;
            crate::syscall::write_user_u32(arg, n)
                .map(|_| SUCCESS)
                .unwrap_or_else(|e| e)
        }
        _ => EINVAL,
    }
}

/// mprotect システムコール
///
/// x86_64 の現在のユーザー保護モデルでは READ は常に許可単位になるため、
/// ここでは READ/WRITE/EXEC の組み合わせのうち W+X を拒否しつつ、
/// 既存マッピングの WRITABLE / NX を更新する。
pub fn mprotect(addr: u64, len: u64, prot: u64) -> u64 {
    const PROT_READ: u64 = 0x1;
    const PROT_WRITE: u64 = 0x2;
    const PROT_EXEC: u64 = 0x4;
    const SUPPORTED_MASK: u64 = PROT_READ | PROT_WRITE | PROT_EXEC;
    const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;

    if len == 0 {
        return SUCCESS;
    }
    if (prot & !SUPPORTED_MASK) != 0 {
        return EINVAL;
    }
    if addr == 0 || addr > USER_SPACE_END {
        return EINVAL;
    }

    let end_inclusive = match addr.checked_add(len.saturating_sub(1)) {
        Some(v) if v <= USER_SPACE_END => v,
        _ => return EINVAL,
    };
    let start = addr & !0xfffu64;
    let length = match (end_inclusive & !0xfffu64)
        .checked_add(4096)
        .and_then(|end| end.checked_sub(start))
    {
        Some(v) if v != 0 => v,
        _ => return EINVAL,
    };

    let present = prot != 0;
    let writable = (prot & PROT_WRITE) != 0;
    let executable = (prot & PROT_EXEC) != 0;
    if present && writable && executable {
        return EINVAL;
    }

    let pid = match current_pid() {
        Some(p) => p,
        None => return ESRCH,
    };
    let table_phys = match crate::task::with_process(pid, |p| p.page_table()).flatten() {
        Some(pt) => pt,
        None => return EINVAL,
    };

    match crate::mem::paging::protect_user_range_in_table(
        table_phys, start, length, present, writable, executable,
    ) {
        Ok(()) => SUCCESS,
        Err(crate::Kernel::Memory(crate::result::Memory::NotMapped)) => EFAULT,
        Err(crate::Kernel::Memory(crate::result::Memory::OutOfMemory)) => ENOMEM,
        Err(crate::Kernel::Memory(crate::result::Memory::PermissionDenied))
        | Err(crate::Kernel::Memory(crate::result::Memory::InvalidAddress))
        | Err(crate::Kernel::Memory(crate::result::Memory::AlignmentError))
        | Err(crate::Kernel::InvalidParam) => EINVAL,
        Err(_) => EFAULT,
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
            let fd = match crate::syscall::read_user_i32(base) {
                Ok(fd) => fd,
                Err(_) => continue,
            };
            let events = match crate::syscall::read_user_u16(base + 4) {
                Ok(events) => events,
                Err(_) => continue,
            };
            let mut revents: u16 = 0;
            if fd >= 0 {
                if (events & (POLLIN | POLLRDNORM)) != 0 && stdin_ready_for_fd(fd) {
                    revents |= POLLIN;
                }
                if (events & (POLLOUT | POLLWRNORM)) != 0 && stdout_ready(fd) {
                    revents |= POLLOUT;
                }
            }
            if crate::syscall::write_user_u16(base + 6, revents).is_err() {
                continue;
            }
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
pub fn ppoll(
    fds_ptr: u64,
    nfds: u64,
    timeout_ptr: u64,
    _sigmask_ptr: u64,
    _sigsetsize: u64,
) -> u64 {
    let timeout_ms_u64 = if timeout_ptr == 0 {
        u64::MAX
    } else {
        if !crate::syscall::validate_user_ptr(timeout_ptr, 16) {
            return EFAULT;
        }
        let sec = match crate::syscall::read_user_i64(timeout_ptr) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let nsec = match crate::syscall::read_user_i64(timeout_ptr + 8) {
            Ok(v) => v,
            Err(e) => return e,
        };
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
        let sec = match crate::syscall::read_user_i64(timeout_ptr) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let nsec = match crate::syscall::read_user_i64(timeout_ptr + 8) {
            Ok(v) => v,
            Err(e) => return e,
        };
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
            if fdset_clear_all(readfds_ptr, set_len).is_err() {
                return 0;
            }
            for fd in ready_fds {
                if fdset_set(readfds_ptr, fd).is_ok() {
                    ready_count += 1;
                }
            }
        }
        if writefds_ptr != 0 {
            let mut ready_fds: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
            for &fd in &write_interest {
                if stdout_ready(fd as i32) {
                    ready_fds.push(fd);
                }
            }
            if fdset_clear_all(writefds_ptr, set_len).is_err() {
                return 0;
            }
            for fd in ready_fds {
                if fdset_set(writefds_ptr, fd).is_ok() {
                    ready_count += 1;
                }
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
            let _ = fdset_clear_all(readfds_ptr, set_len);
        }
        if writefds_ptr != 0 {
            let _ = fdset_clear_all(writefds_ptr, set_len);
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
    let mut buf = [0u8; UTSNAME_SIZE as usize];
    for (i, f) in fields.iter().enumerate() {
        let off = i * FIELD_LEN;
        let n = f.len().min(FIELD_LEN - 1);
        buf[off..off + n].copy_from_slice(&f[..n]);
    }
    crate::syscall::copy_to_user(buf_ptr, &buf)
        .map(|_| SUCCESS)
        .unwrap_or_else(|e| e)
}

/// nanosleep システムコール
///
/// struct timespec { tv_sec: i64, tv_nsec: i64 } を受け取りスリープする。
pub fn nanosleep(req_ptr: u64, _rem_ptr: u64) -> u64 {
    if req_ptr == 0 || !crate::syscall::validate_user_ptr(req_ptr, 16) {
        return EINVAL;
    }
    let secs = match crate::syscall::read_user_i64(req_ptr) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let nsecs = match crate::syscall::read_user_i64(req_ptr + 8) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if secs < 0 || nsecs < 0 || nsecs >= 1_000_000_000 {
        return EINVAL;
    }
    let total_ms = (secs as u64) * 1000 + (nsecs as u64) / 1_000_000;
    if total_ms > 0 {
        crate::syscall::process::sleep(total_ms);
    }
    SUCCESS
}

/// getrlimit システムコール（リソース上限を無限大で返す）
pub fn getrlimit(_resource: u64, rlim_ptr: u64) -> u64 {
    if rlim_ptr == 0 || !crate::syscall::validate_user_ptr(rlim_ptr, 16) {
        return EINVAL;
    }
    let mut buf = [0u8; 16];
    buf[..8].copy_from_slice(&u64::MAX.to_ne_bytes());
    buf[8..].copy_from_slice(&u64::MAX.to_ne_bytes());
    crate::syscall::copy_to_user(rlim_ptr, &buf)
        .map(|_| SUCCESS)
        .unwrap_or_else(|e| e)
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
