#![no_std]
#![no_main]

#[macro_use]
extern crate std;
extern crate alloc;

use core::panic::PanicInfo;
use std::{process, thread};
use std::fs::File;
use alloc::string::String;

/// ユーザーアプリのエントリーポイント
#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::heap::init();
    println!("TestApp Started with swift_std (High Level API)!");

    // ファイルを開く
    let filename = "readme.txt";
    println!("Opening {}...", filename);

    // Std-like API usage
    match File::open(filename) {
        Ok(mut file) => {
            println!("File opened successfully.");

            let mut content = String::new();
            match file.read_to_string(&mut content) {
                Ok(len) => {
                    println!("Read {} bytes:\n---", len);
                    println!("{}", content);
                    println!("---");
                },
                Err(e) => {
                     println!("Failed to read file: {:?}", e);
                }
            }

            // Write test
            let msg = "\nAppended via File API!";
            match file.write(msg.as_bytes()) {
                 Ok(_) => println!("Successfully appended message."),
                 Err(_) => println!("Failed to append message."),
            }

            // Drop will close the file
        },
        Err(e) => {
            println!("Failed to open file: {:?}", e);
        }
    }
    
    println!("TestApp finished.");
    process::exit(0);
}

/// パニックハンドラ
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("PANIC in user space: {}", info);
    loop {
        thread::yield_now();
    }
}
