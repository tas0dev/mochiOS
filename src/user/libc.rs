pub unsafe extern "C" fn write(fd: i32, buf: *const u8, count: usize) -> isize {
    let slice = core::slice::from_raw_parts(buf, count);
    let ret = crate::io::write(fd as u64, slice);
    
    if ret == u64::MAX {
        -1
    } else {
        ret as isize
    }
}

pub unsafe extern "C" fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
    let slice = core::slice::from_raw_parts_mut(buf, count);
    let ret = crate::io::read(fd as u64, slice);
    
    if ret == u64::MAX {
        -1
    } else {
        ret as isize
    }
}

pub unsafe extern "C" fn memalign(alignment: usize, size: usize) -> *mut u8 {
    // newlib の malloc に委譲 (newlib は内部で _sbrk を呼ぶ)
    extern "C" {
        fn malloc(size: usize) -> *mut u8;
    }
    // アライメントが標準以下なら malloc を使う
    let ptr = malloc(size + alignment);
    if ptr.is_null() {
        return core::ptr::null_mut();
    }
    let addr = ptr as usize;
    let aligned = (addr + alignment - 1) & !(alignment - 1);
    aligned as *mut u8
}

pub unsafe extern "C" fn free(ptr: *mut u8) {
    // TODO: メモリ解放のシステムコールを実装
}

pub unsafe extern "C" fn realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    // TODO: リサイズのシステムコールを実装
    ptr
}

pub unsafe extern "C" fn open(_path: *const u8, _flags: i32) -> i32 { -1 }

pub unsafe extern "C" fn close(fd: i32) -> i32 {
    crate::io::close(fd as u64) as i32
}