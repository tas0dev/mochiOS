pub unsafe extern "C" fn write(fd: i32, buf: *const u8, count: usize) -> isize {
    let slice = core::slice::from_raw_parts(buf, count);
    let ret = crate::io::write(fd as u64, slice);

    if (ret as i64) < 0 {
        -1
    } else {
        ret as isize
    }
}

pub unsafe extern "C" fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
    let slice = core::slice::from_raw_parts_mut(buf, count);
    let ret = crate::io::read(fd as u64, slice);

    if (ret as i64) < 0 {
        -1
    } else {
        ret as isize
    }
}

pub unsafe extern "C" fn memalign(alignment: usize, size: usize) -> *mut u8 {
    extern "C" {
        #[link_name = "memalign"]
        fn c_memalign(alignment: usize, size: usize) -> *mut u8;
    }
    c_memalign(alignment, size)
}

pub unsafe extern "C" fn free(ptr: *mut u8) {
    extern "C" {
        #[link_name = "free"]
        fn c_free(ptr: *mut u8);
    }
    c_free(ptr);
}

pub unsafe extern "C" fn realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    extern "C" {
        #[link_name = "realloc"]
        fn c_realloc(ptr: *mut u8, size: usize) -> *mut u8;
    }
    c_realloc(ptr, size)
}

pub unsafe extern "C" fn open(_path: *const u8, _flags: i32) -> i32 { -1 }

pub unsafe extern "C" fn close(fd: i32) -> i32 {
    crate::io::close(fd as u64) as i32
}

#[inline]
pub fn inb(port: u16) -> u8 {
    crate::port::inb(port)
}

#[inline]
pub fn outb(port: u16, value: u8) {
    crate::port::outb(port, value)
}

#[inline]
pub fn inw(port: u16) -> u16 {
    crate::port::inw(port)
}

#[inline]
pub fn outw(port: u16, value: u16) {
    crate::port::outw(port, value)
}

#[inline]
pub fn inl(port: u16) -> u32 {
    crate::port::inl(port)
}

#[inline]
pub fn outl(port: u16, value: u32) {
    crate::port::outl(port, value)
}
