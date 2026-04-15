//! Newlib サポート用のシステムコールグルーコード

use super::sys::{syscall1, syscall2, syscall3, SyscallNumber};
use core::sync::atomic::{AtomicBool, Ordering};

static SBRK_LOCK: AtomicBool = AtomicBool::new(false);

unsafe fn set_errno_from_ret(ret: i64) {
    if ret >= 0 {
        return;
    }
    unsafe extern "C" {
        fn __errno_location() -> *mut i32;
    }
    let errno = (-ret) as i32;
    let p = unsafe { __errno_location() };
    if !p.is_null() {
        unsafe { *p = errno };
    }
}

struct SbrkLockGuard;

impl SbrkLockGuard {
    fn lock() -> Self {
        while SBRK_LOCK
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        Self
    }
}

impl Drop for SbrkLockGuard {
    fn drop(&mut self) {
        SBRK_LOCK.store(false, Ordering::Release);
    }
}

#[no_mangle]
pub extern "C" fn _write(fd: i32, buf: *const u8, len: usize) -> isize {
    let ret = syscall3(SyscallNumber::Write as u64, fd as u64, buf as u64, len as u64) as i64;
    if ret < 0 {
        unsafe { set_errno_from_ret(ret) };
        -1
    } else {
        ret as isize
    }
}

#[no_mangle]
pub extern "C" fn write(fd: i32, buf: *const u8, len: usize) -> isize {
    _write(fd, buf, len)
}

#[no_mangle]
pub extern "C" fn _read(fd: i32, buf: *mut u8, len: usize) -> isize {
    let ret = syscall3(SyscallNumber::Read as u64, fd as u64, buf as u64, len as u64) as i64;
    if ret < 0 {
        unsafe { set_errno_from_ret(ret) };
        -1
    } else {
        ret as isize
    }
}

#[no_mangle]
pub extern "C" fn read(fd: i32, buf: *mut u8, len: usize) -> isize {
    _read(fd, buf, len)
}

#[no_mangle]
pub extern "C" fn _close(fd: i32) -> i32 {
    let ret = syscall1(SyscallNumber::Close as u64, fd as u64) as i64;
    if ret < 0 {
        unsafe { set_errno_from_ret(ret) };
        -1
    } else {
        ret as i32
    }
}

#[no_mangle]
pub extern "C" fn close(fd: i32) -> i32 {
    _close(fd)
}

#[no_mangle]
pub extern "C" fn _open(path: *const u8, flags: i32, _mode: i32) -> i32 {
    let ret = syscall2(SyscallNumber::Open as u64, path as u64, flags as u64) as i64;
    if ret < 0 {
        unsafe { set_errno_from_ret(ret) };
        -1
    } else {
        ret as i32
    }
}

#[no_mangle]
pub extern "C" fn _lseek(fd: i32, offset: isize, whence: i32) -> isize {
    let ret = syscall3(
        SyscallNumber::Lseek as u64,
        fd as u64,
        offset as u64,
        whence as u64,
    ) as i64;
    if ret < 0 {
        unsafe { set_errno_from_ret(ret) };
        -1
    } else {
        ret as isize
    }
}

#[no_mangle]
pub extern "C" fn lseek(fd: i32, offset: isize, whence: i32) -> isize {
    _lseek(fd, offset, whence)
}

#[no_mangle]
pub extern "C" fn _exit(code: i32) -> ! {
    syscall1(SyscallNumber::Exit as u64, code as u64);
    loop {}
}

// exit は libc にあるので定義しなくてよいかも？でも _exit を呼ぶはず。
// ただし crt0 から呼ばれるのは _exit だったりする。

#[no_mangle]
pub extern "C" fn _fstat(fd: i32, stat: *mut u8) -> i32 {
    let ret = syscall2(SyscallNumber::Fstat as u64, fd as u64, stat as u64) as i64;
    if ret < 0 {
        unsafe { set_errno_from_ret(ret) };
        -1
    } else {
        ret as i32
    }
}

#[no_mangle]
pub extern "C" fn fstat(fd: i32, stat: *mut u8) -> i32 {
    _fstat(fd, stat)
}

#[no_mangle]
pub extern "C" fn _ioctl(fd: i32, request: u64, arg: u64) -> i32 {
    let ret = syscall3(
        SyscallNumber::Ioctl as u64,
        fd as u64,
        request,
        arg,
    ) as i64;
    if ret < 0 {
        unsafe { set_errno_from_ret(ret) };
        -1
    } else {
        ret as i32
    }
}

#[no_mangle]
pub extern "C" fn _isatty(fd: i32) -> i32 {
    const TCGETS: u64 = 0x5401;
    const XCGETA: u64 = ((b'x' as u64) << 8) | 1;

    if fd < 0 {
        return 0;
    }

    // Linux系 termios ioctl
    let mut termios = [0u8; 36];
    let ret = syscall3(
        SyscallNumber::Ioctl as u64,
        fd as u64,
        TCGETS,
        termios.as_mut_ptr() as u64,
    ) as i64;
    if ret >= 0 {
        return 1;
    }

    // newlib(sysvi386) 互換 ioctl
    let mut termio = [0u8; 18];
    let ret = syscall3(
        SyscallNumber::Ioctl as u64,
        fd as u64,
        XCGETA,
        termio.as_mut_ptr() as u64,
    ) as i64;
    if ret >= 0 { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn isatty(fd: i32) -> i32 {
    _isatty(fd)
}

#[no_mangle]
pub extern "C" fn _sbrk(incr: isize) -> *mut u8 {
    let _sbrk_guard = SbrkLockGuard::lock();

    // brk は mmap/MMIO マッピングでも更新されるため、ユーザー側で末端を
    // キャッシュすると整合性が壊れてヒープ破壊につながる。
    // 毎回 brk(0) で現在値を取得してから更新する。
    let cur = syscall1(SyscallNumber::Brk as u64, 0);
    if cur == 0 || cur > 0xffff_ffff_ffff_f000 {
        return -1_isize as *mut u8;
    }
    let old_heap_end = cur;

    // 安全側に倒して縮小は未サポートにする（MMIO 併用時の破壊回避）。
    if incr < 0 {
        return -1_isize as *mut u8;
    }
    if incr == 0 {
        return old_heap_end as *mut u8;
    }

    let incr_u64 = incr as u64;
    let new_heap_end = match old_heap_end.checked_add(incr_u64) {
        Some(v) => v,
        None => return -1_isize as *mut u8,
    };
    let ret = syscall1(SyscallNumber::Brk as u64, new_heap_end);
    if ret == new_heap_end {
        old_heap_end as *mut u8
    } else {
        -1_isize as *mut u8
    }
}

#[no_mangle]
pub extern "C" fn sbrk(incr: isize) -> *mut u8 {
    _sbrk(incr)
}

#[no_mangle]
pub extern "C" fn _getpid() -> i32 {
    syscall1(SyscallNumber::GetPid as u64, 0) as i32
}

#[no_mangle]
pub extern "C" fn getpid() -> i32 {
    _getpid()
}

#[no_mangle]
pub extern "C" fn _kill(_pid: i32, _sig: i32) -> i32 {
    // 未実装
    -1
}

#[no_mangle]
pub extern "C" fn kill(pid: i32, sig: i32) -> i32 {
    _kill(pid, sig)
}
