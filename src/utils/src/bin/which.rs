#![no_std]
#![no_main]

use swiftlib::io;

#[no_mangle]
pub extern "C" fn main(argc: i32, argv: *const *const u8) -> i32 {
    if argc < 2 {
        io::print("Usage: which <command>\n");
        return 1;
    }
    
    let cmd = unsafe {
        let arg_ptr = *argv.offset(1);
        if arg_ptr.is_null() {
            return 1;
        }
        let mut len = 0;
        while *arg_ptr.offset(len) != 0 {
            len += 1;
        }
        match core::str::from_utf8(core::slice::from_raw_parts(arg_ptr, len as usize)) {
            Ok(s) => s,
            Err(_) => return 1,
        }
    };
    
    // PATH はデフォルトで /bin
    let path = "/bin";
    
    // コマンドのフルパスを構築
    let mut full_path_buf = [0u8; 256];
    let path_bytes = path.as_bytes();
    let cmd_bytes = cmd.as_bytes();
    
    let mut pos = 0;
    for &b in path_bytes {
        if pos >= 255 {
            break;
        }
        full_path_buf[pos] = b;
        pos += 1;
    }
    
    if pos < 255 {
        full_path_buf[pos] = b'/';
        pos += 1;
    }
    
    for &b in cmd_bytes {
        if pos >= 255 {
            break;
        }
        full_path_buf[pos] = b;
        pos += 1;
    }
    
    // .elf 拡張子を追加
    if pos + 4 < 256 {
        full_path_buf[pos] = b'.';
        full_path_buf[pos + 1] = b'e';
        full_path_buf[pos + 2] = b'l';
        full_path_buf[pos + 3] = b'f';
        pos += 4;
    }
    
    if let Ok(full_path) = core::str::from_utf8(&full_path_buf[..pos]) {
        io::print(full_path);
        io::print("\n");
    }
    
    0
}
