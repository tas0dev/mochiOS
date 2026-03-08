use crate::syscall::ENODATA;

/// PS/2 キーボードから rawスキャンコードを1バイト読み取り
/// バッファが空なら ENODATA を返す（変換はユーザー空間で行う）
pub fn read_char() -> u64 {
    match crate::util::ps2kbd::pop_scancode() {
        Some(sc) => sc as u64,
        None => ENODATA,
    }
}
