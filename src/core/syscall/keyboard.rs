use crate::syscall::ENODATA;

/// PS/2 キーボードから rawスキャンコードを1バイト読み取り
/// バッファが空なら ENODATA を返す（変換はユーザー空間で行う）
pub fn read_char() -> u64 {
    match crate::util::ps2kbd::pop_scancode() {
        Some(sc) => sc as u64,
        None => ENODATA,
    }
}

/// PS/2 キーボードから rawスキャンコードを1バイト読み取る（ブロッキング版）
///
/// バッファが空であれば、スキャンコードが届くまでスレッドをスリープして待機する。
/// IPC recv_blocking と同じ「登録→再確認→眠る」パターンで競合を回避する。
pub fn read_char_blocking() -> u8 {
    let tid = match crate::task::current_thread_id() {
        Some(id) => id,
        // カーネルスレッドからの呼び出し（通常は起きない）: スピンで待つ
        None => loop {
            if let Some(sc) = crate::util::ps2kbd::pop_scancode() {
                return sc;
            }
            crate::task::yield_now();
        },
    };

    loop {
        // waiter を登録してから pop を再試行することで、登録後に届いたスキャンコードを見逃さない
        crate::util::ps2kbd::register_waiter(tid.as_u64());

        if let Some(sc) = crate::util::ps2kbd::pop_scancode() {
            // データがあった → 起床不要なので waiter をクリアして返す
            crate::util::ps2kbd::unregister_waiter(tid.as_u64());
            return sc;
        }

        // データなし → pending_wakeup がなければスリープして yield
        if crate::task::sleep_thread_unless_woken(tid) {
            crate::task::yield_now();
            // 起床後にループしてデータを再確認
        }
        // pending_wakeup で即起床した場合もループして再確認
    }
}
