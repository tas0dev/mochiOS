#![no_std]
#![no_main]

#[repr(C)]
pub struct McxDiskOps {
    pub probe: extern "C" fn() -> i32,
}

extern "C" fn probe() -> i32 {
    0
}

static DISK_OPS: McxDiskOps = McxDiskOps { probe };

#[no_mangle]
pub extern "C" fn mochi_module_init() -> *const McxDiskOps {
    &DISK_OPS
}

#[used]
static KEEP_INIT_REF: extern "C" fn() -> *const McxDiskOps = mochi_module_init;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
