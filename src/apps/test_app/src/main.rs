#![no_std]
#![no_main]

extern crate test_app;
use core::panic::PanicInfo;

use test_app::{yield_now, print, exit, getpid, gettid, sleep, get_ticks};

/// ユーザーアプリのエントリーポイント
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Writeシステムコールのテスト
    print("Hello from user space!\n");
    print("Testing syscalls...\n");

    // GetPidとGetTidのテスト
    let pid = getpid();
    let tid = gettid();
    print("Process ID: ");
    print_u64(pid);
    print("\nThread ID: ");
    print_u64(tid);
    print("\n");

    // GetTicksのテスト
    let ticks = get_ticks();
    print("Current ticks: ");
    print_u64(ticks);
    print("\n");

    let mut counter = 0u64;

    loop {
        // カウンターをインクリメント
        counter = counter.wrapping_add(1);

        // 10000回ごとにメッセージを出力
        if counter % 10000 == 0 {
            print("Working... counter = ");
            print_u64(counter);
            print("\n");

            // Sleepのテスト（100ミリ秒）
            print("Sleeping for 100ms...\n");
            sleep(100);
            print("Woke up!\n");
        }

        // 30000回でループを抜ける
        if counter >= 30000 {
            break;
        }
    }

    // 終了メッセージ
    print("User app finished. Exiting...\n");

    // exitシステムコールでプロセスを終了
    exit(0);
}

/// 数値を文字列として出力（簡易実装）
fn print_u64(mut num: u64) {
    if num == 0 {
        print("0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;

    while num > 0 {
        buf[i] = (num % 10) as u8 + b'0';
        num /= 10;
        i += 1;
    }

    // 逆順で出力
    while i > 0 {
        i -= 1;
        let s = core::str::from_utf8(&buf[i..i+1]).unwrap();
        print(s);
    }
}

/// パニックハンドラ
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    print("PANIC in user space!\n");
    loop {
        yield_now();
    }
}
