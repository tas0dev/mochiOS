//! PS/2 キーボードドライバ (カーネル側)
//!
//! IRQ1 割り込みハンドラからスキャンコードを受け取りFIFOに蓄積する。
//! 変換ロジックはユーザー空間 (shell.service) が担当する。

use core::sync::atomic::{AtomicU64, Ordering};
use super::fifo::Fifo;

/// rawスキャンコードのバッファ (割り込みハンドラ ↔ syscall)
pub static SCANCODE_BUF: Fifo<u8, 256> = Fifo::new();

/// read(0, ...) でブロッキング待機しているスレッドのID（0 = 待ちなし）
static KEYBOARD_WAITER: AtomicU64 = AtomicU64::new(0);

/// IRQ1 ハンドラから呼ぶ: rawスキャンコードをバッファへ積み、待機スレッドを起床させる
pub fn push_scancode(scancode: u8) {
    let _ = SCANCODE_BUF.push(scancode);

    // ブロッキング read で眠っているスレッドがいれば起床させる
    let waiter = KEYBOARD_WAITER.swap(0, Ordering::AcqRel);
    if waiter != 0 {
        crate::task::wake_thread(crate::task::ThreadId::from_u64(waiter));
    }
}

/// `KeyboardRead` syscall から呼ぶ: rawスキャンコードを1バイト取り出す
pub fn pop_scancode() -> Option<u8> {
    SCANCODE_BUF.pop()
}

/// ブロッキング read 用: 現在のスレッドをwaiterとして登録する
///
/// スキャンコードが届いたとき `push_scancode` が起床させる。
/// 呼び出し前に waiter 登録 → pop 再試行 → 眠る、の順にすること（競合回避）。
pub fn register_waiter(tid: u64) {
    KEYBOARD_WAITER.store(tid, Ordering::Release);
}

/// waiter 登録をキャンセルする（眠らずに済んだ場合のクリーンアップ用）
pub fn unregister_waiter(tid: u64) {
    let _ = KEYBOARD_WAITER.compare_exchange(tid, 0, Ordering::AcqRel, Ordering::Relaxed);
}
