use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU16, Ordering};

#[repr(C)]
pub struct McxDiskOps {
    pub probe: extern "C" fn() -> i32,
}

static LOADED: AtomicBool = AtomicBool::new(false);
static VERSION: AtomicU16 = AtomicU16::new(0);
static DISK_OPS_PTR: AtomicPtr<McxDiskOps> = AtomicPtr::new(core::ptr::null_mut());

pub fn register(ops: *const McxDiskOps, version: u16) -> bool {
    if ops.is_null() {
        return false;
    }
    DISK_OPS_PTR.store(ops as *mut McxDiskOps, Ordering::Release);
    VERSION.store(version, Ordering::Release);
    LOADED.store(true, Ordering::Release);
    true
}

pub fn is_loaded() -> bool {
    LOADED.load(Ordering::Acquire)
}

#[allow(dead_code)]
pub fn version() -> u16 {
    VERSION.load(Ordering::Acquire)
}

#[allow(dead_code)]
pub fn probe() -> i32 {
    let ops = DISK_OPS_PTR.load(Ordering::Acquire);
    if ops.is_null() {
        return -38;
    }
    unsafe { ((*ops).probe)() }
}
