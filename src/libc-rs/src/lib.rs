#![no_std]
#![allow(non_camel_case_types)]

pub mod prelude {
    pub use core::prelude::v1::*;
}

pub type c_int = i32;
pub type c_long = i64;
pub type c_ulong = u64;
pub type c_char = i8;
pub type c_uchar = u8;
pub type c_uint = u32;
pub type c_void = core::ffi::c_void;
pub type size_t = usize;
pub type ssize_t = isize;
pub type off_t = i64;
pub type regoff_t = i64;
pub type __u64 = u64;
pub type __s64 = i64;
pub type Ioctl = u64;
pub type pid_t = i32;
pub type uid_t = u32;
pub type gid_t = u32;
pub type mode_t = u32;
pub type dev_t = u64;
pub type ino_t = u64;
pub type nlink_t = u64;
pub type blksize_t = i64;
pub type blkcnt_t = i64;
pub type time_t = i64;
pub type nfds_t = u64;

pub enum sigset_t {}
pub enum siginfo_t {}
pub enum sem_t {}
pub enum stack_t {}
pub enum regex_t {}
pub enum msghdr {}
pub enum cmsghdr {}
pub enum mmsghdr {}
pub enum termios {}
pub enum termios2 {}
pub struct sysinfo {}
pub struct statfs {}
pub struct statfs64 {}
pub struct statvfs64 {}
pub struct stat64 {}
pub struct file_clone_range {}

pub const T_TYPE: u32 = 0;
pub const _IOC_SIZESHIFT: u32 = 0;

pub unsafe fn ioctl(_fd: c_int, _request: Ioctl, ...) -> c_int { 0 }
pub unsafe fn CMSG_FIRSTHDR(_mhdr: *const msghdr) -> *mut cmsghdr { core::ptr::null_mut() }
pub unsafe fn CMSG_DATA(_cmsg: *const cmsghdr) -> *mut c_uchar { core::ptr::null_mut() }
pub const fn CMSG_ALIGN(_len: usize) -> usize { 0 }
pub const fn CMSG_SPACE(_len: c_uint) -> c_uint { 0 }
pub const fn CMSG_LEN(_len: c_uint) -> c_uint { 0 }
