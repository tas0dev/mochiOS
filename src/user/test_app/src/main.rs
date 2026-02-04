#![no_std]
#![no_main]

extern crate test_app;
use core::panic::PanicInfo;

use test_app::yield_now;

/// ユーザーアプリのエントリーポイント
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 初期化処理
    // システムコールを使ってタスクのライフサイクルをテスト

    let mut counter = 0u64;

    loop {
        // カウンターをインクリメント
        counter = counter.wrapping_add(1);

        // 1000回ごとにyieldを呼ぶ
        if counter % 1000 == 0 {
            yield_now();
        }

        // 100000回でループを抜ける（テスト用）
        if counter >= 100000 {
            break;
        }
    }

    // 無限ループでyield
    loop {
        yield_now();
    }
}

/// パニックハンドラ
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        yield_now();
    }
}
