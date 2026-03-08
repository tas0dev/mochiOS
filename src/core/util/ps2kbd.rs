//! PS/2 キーボードドライバ (カーネル側)
//!
//! IRQ1 割り込みハンドラからスキャンコードを受け取りFIFOに蓄積する。
//! 変換ロジックはユーザー空間 (shell.service) が担当する。

use super::fifo::Fifo;

/// rawスキャンコードのバッファ (割り込みハンドラ ↔ syscall)
pub static SCANCODE_BUF: Fifo<u8, 256> = Fifo::new();

/// IRQ1 ハンドラから呼ぶ: rawスキャンコードをバッファへ
pub fn push_scancode(scancode: u8) {
    let _ = SCANCODE_BUF.push(scancode);
}

/// `KeyboardRead` syscall から呼ぶ: rawスキャンコードを1バイト取り出す
pub fn pop_scancode() -> Option<u8> {
    SCANCODE_BUF.pop()
}
