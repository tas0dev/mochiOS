#![no_std]
#![no_main]

use core::panic::PanicInfo;

extern crate test_app;
use test_app::{yield_now, get_ticks};

/// ユーザーアプリのエントリーポイント
///
/// このアプリケーションは以下を実行します:
/// 1. タイマーティック値を取得
/// 2. カウンターをインクリメント
/// 3. 定期的にyieldして他のタスクに譲る
#[no_mangle]
pub extern "C" fn _start() -> ! {
    let start_ticks = get_ticks();
    let mut counter = 0u64;
    let mut last_yield_counter = 0u64;

    loop {
        counter = counter.wrapping_add(1);

        // 1000回ごとにyieldを呼ぶ
        if counter - last_yield_counter >= 1000 {
            yield_now();
            last_yield_counter = counter;
        }

        // 1000000回でループを抜ける（テスト用）
        if counter >= 1000000 {
            break;
        }
    }

    // 終了前に最終的な統計を計算
    let end_ticks = get_ticks();
    let elapsed = end_ticks.wrapping_sub(start_ticks);

    // TODO: write()システムコールが実装されたら結果を出力
    // write(1, "Task completed\n");

    // 無限ループでyield（プロセスを維持）
    loop {
        yield_now();
    }
}

/// パニックハンドラ
///
/// パニック時は単純にyieldを繰り返してCPUを消費しない
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        yield_now();
    }
}
