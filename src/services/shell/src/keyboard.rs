//! PS/2 キーボードドライバ (ユーザー空間)
//!
//! カーネルから rawスキャンコード (セット1) を受け取り
//! ASCII 文字に変換する。Shift / CapsLock の状態を保持する。

use swiftlib::keyboard;

/// スキャンコードセット1 → ASCII（通常）
#[rustfmt::skip]
const MAP_NORMAL: [u8; 128] = [
    0,    0x1B, b'1', b'2', b'3', b'4', b'5', b'6',   // 0x00–0x07
    b'7', b'8', b'9', b'0', b'-', b'=', 0x08, b'\t',  // 0x08–0x0F
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i',   // 0x10–0x17
    b'o', b'p', b'[', b']', b'\n', 0,   b'a', b's',   // 0x18–0x1F
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',   // 0x20–0x27
    b':', b'`', 0,   b'\\',b'z', b'x', b'c', b'v',    // 0x28–0x2F
    b'b', b'n', b'm', b',', b'.', b'/', 0,   b'*',    // 0x30–0x37
    0,    b' ', 0,    0,    0,    0,    0,    0,        // 0x38–0x3F
    0,    0,    0,    0,    0,    0,    0,    b'7',     // 0x40–0x47
    b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1',   // 0x48–0x4F
    b'2', b'3', b'0', b'.', 0,    0,    0,    0,       // 0x50–0x57
    0,    0,    0,    0,    0,    0,    0,    0,        // 0x58–0x5F
    0,    0,    0,    0,    0,    0,    0,    0,        // 0x60–0x67
    0,    0,    0,    0,    0,    0,    0,    0,        // 0x68–0x6F
    0,    0,    0,    0,    0,    0,    0,    0,        // 0x70–0x77
    0,    0,    0,    0,    0,    0,    0,    0,        // 0x78–0x7F
];

/// スキャンコードセット1 → ASCII（Shift押下時）
#[rustfmt::skip]
const MAP_SHIFT: [u8; 128] = [
    0,    0x1B, b'!', b'@', b'#', b'$', b'%', b'^',   // 0x00–0x07
    b'&', b'*', b'(', b')', b'_', b'+', 0x08, b'\t',  // 0x08–0x0F
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I',   // 0x10–0x17
    b'O', b'P', b'{', b'}', b'\n', 0,   b'A', b'S',   // 0x18–0x1F
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',   // 0x20–0x27
    b'*', b'~', 0,   b'|', b'Z', b'X', b'C', b'V',    // 0x28–0x2F
    b'B', b'N', b'M', b'<', b'>', b'?', 0,   b'*',    // 0x30–0x37
    0,    b' ', 0,    0,    0,    0,    0,    0,        // 0x38–0x3F
    0,    0,    0,    0,    0,    0,    0,    b'7',     // 0x40–0x47
    b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1',   // 0x48–0x4F
    b'2', b'3', b'0', b'.', 0,    0,    0,    0,       // 0x50–0x57
    0,    0,    0,    0,    0,    0,    0,    0,        // 0x58–0x5F
    0,    0,    0,    0,    0,    0,    0,    0,        // 0x60–0x67
    0,    0,    0,    0,    0,    0,    0,    0,        // 0x68–0x6F
    0,    0,    0,    0,    0,    0,    0,    0,        // 0x70–0x77
    0,    0,    0,    0,    0,    0,    0,    0,        // 0x78–0x7F
];

// スキャンコード定数
const SC_LSHIFT: u8 = 0x2A;
const SC_RSHIFT: u8 = 0x36;
const SC_CAPSLOCK: u8 = 0x3A;
const SC_RELEASE: u8 = 0x80; // リリースフラグ (bit 7)

/// PS/2 キーボードドライバ
pub struct Ps2Keyboard {
    shift: bool,
    caps: bool,
}

impl Ps2Keyboard {
    pub const fn new() -> Self {
        Ps2Keyboard { shift: false, caps: false }
    }

    /// カーネルバッファからスキャンコードを取得して ASCII に変換する。
    /// キー入力がなければ None を返す。
    pub fn read(&mut self) -> Option<u8> {
        loop {
            let sc = keyboard::read_scancode()?;

            // キーリリース
            if sc & SC_RELEASE != 0 {
                let make = sc & !SC_RELEASE;
                if make == SC_LSHIFT || make == SC_RSHIFT {
                    self.shift = false;
                }
                continue; // リリースイベントは文字を生成しない
            }

            // 修飾キー
            match sc {
                SC_LSHIFT | SC_RSHIFT => { self.shift = true; continue; }
                SC_CAPSLOCK => { self.caps = !self.caps; continue; }
                _ => {}
            }

            let idx = sc as usize;
            if idx >= 128 {
                continue;
            }

            // Shift と CapsLock を組み合わせて文字を決定
            let use_shift = self.shift ^ (self.caps && MAP_NORMAL[idx].is_ascii_alphabetic());
            let ch = if use_shift { MAP_SHIFT[idx] } else { MAP_NORMAL[idx] };

            if ch != 0 {
                return Some(ch);
            }
        }
    }
}
