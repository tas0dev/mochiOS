use crate::interrupt::spinlock::SpinLock;

use super::{EAGAIN, EFAULT, EINVAL};

const MAX_THREADS: usize = crate::task::ThreadQueue::MAX_THREADS;
const MAILBOX_CAP: usize = 64;
const MAX_MSG_SIZE: usize = 576; // DiskRequest(544) と DiskResponse(528) を収容できる最小サイズ

#[derive(Debug, Clone, Copy)]
pub struct Message {
    from: u64,
    to: u64,
    to_slot: u16,
    to_generation: u64,
    len: usize,
    data: [u8; MAX_MSG_SIZE],
}

impl Message {
    const fn empty() -> Self {
        Self {
            from: 0,
            to: 0,
            to_slot: 0,
            to_generation: 0,
            len: 0,
            data: [0; MAX_MSG_SIZE],
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Mailbox {
    head: usize,
    tail: usize,
    count: usize,
    buf: [Message; MAILBOX_CAP],
    /// メッセージ待ちでスリープ中のスレッドID (0=なし)
    waiter: u64,
}

impl Mailbox {
    const fn new() -> Self {
        Self {
            head: 0,
            tail: 0,
            count: 0,
            buf: [Message::empty(); MAILBOX_CAP],
            waiter: 0,
        }
    }

    fn push(&mut self, msg: Message) -> Result<(), ()> {
        if self.count >= MAILBOX_CAP {
            return Err(());
        }
        self.buf[self.tail] = msg;
        self.tail = (self.tail + 1) % MAILBOX_CAP;
        self.count += 1;
        Ok(())
    }

    fn pop(&mut self) -> Option<Message> {
        if self.count == 0 {
            return None;
        }
        let msg = self.buf[self.head];
        self.head = (self.head + 1) % MAILBOX_CAP;
        self.count -= 1;
        Some(msg)
    }

    /// メッセージを積んだ後、待機中スレッドがいれば返して登録を消す
    fn take_waiter(&mut self) -> u64 {
        let w = self.waiter;
        self.waiter = 0;
        w
    }
}

static MAILBOXES: SpinLock<[Mailbox; MAX_THREADS]> = SpinLock::new([Mailbox::new(); MAX_THREADS]);

/// カーネル内部からIPC送信（ユーザー空間コピー不要）
pub fn send_from_kernel(dest_thread_id: u64, data: &[u8]) -> bool {
    let len = data.len();
    if len > MAX_MSG_SIZE {
        return false;
    }
    let (idx, dest_generation) =
        match crate::task::thread_slot_index_and_generation_by_u64(dest_thread_id) {
            Some(v) => v,
            None => return false,
        };
    if idx >= MAX_THREADS {
        return false;
    }
    let sender = crate::task::current_thread_id()
        .map(|t| t.as_u64())
        .unwrap_or(0);
    let mut msg_data = [0u8; MAX_MSG_SIZE];
    msg_data[..len].copy_from_slice(data);
    let msg = Message {
        from: sender,
        to: dest_thread_id,
        to_slot: idx as u16,
        to_generation: dest_generation,
        len,
        data: msg_data,
    };
    MAILBOXES.lock().get_mut(idx).map_or(false, |mb| {
        if mb.push(msg).is_ok() {
            let waiter = mb.take_waiter();
            if waiter != 0 {
                crate::task::wake_thread(crate::task::ThreadId::from_u64(waiter));
            }
            true
        } else {
            false
        }
    })
}

/// IPC送信
/// arg0: dest_thread_id
/// arg1: buf_ptr
/// arg2: len
pub fn send(dest_thread_id: u64, buf_ptr: u64, len: u64) -> u64 {
    if dest_thread_id == 0 {
        return EINVAL;
    }

    let len = len as usize;
    if len > MAX_MSG_SIZE {
        return EINVAL;
    }
    if len > 0 && buf_ptr == 0 {
        return EFAULT;
    }

    let sender = match crate::task::current_thread_id() {
        Some(id) => id.as_u64(),
        None => return EINVAL,
    };

    let (idx, dest_generation) =
        match crate::task::thread_slot_index_and_generation_by_u64(dest_thread_id) {
            Some(v) => v,
            None => return EINVAL,
        };

    if idx >= MAX_THREADS || idx > (u16::MAX as usize) {
        return EINVAL;
    }

    // NOTE:
    // - 宛先スロットに加えて世代番号をメッセージへ埋め込む。
    // - これにより、送信先終了後に同一スロットへ別スレッドが再利用されても誤配送されない。
    // - 送信時点と受信時点で世代不一致なら古いメッセージとして破棄される。

    // データをユーザー空間からコピー
    let mut data = [0u8; MAX_MSG_SIZE];
    if len > 0 && buf_ptr != 0 {
        if let Err(err) = crate::syscall::copy_from_user(buf_ptr, &mut data[..len]) {
            return err;
        }
    }

    let msg = Message {
        from: sender,
        to: dest_thread_id,
        to_slot: idx as u16,
        to_generation: dest_generation,
        len,
        data,
    };

    let mut boxes = MAILBOXES.lock();
    if boxes[idx].push(msg).is_err() {
        return EAGAIN;
    }
    let waiter = boxes[idx].take_waiter();
    drop(boxes);
    if waiter != 0 {
        crate::task::wake_thread(crate::task::ThreadId::from_u64(waiter));
    }

    0
}

/// IPC受信
/// arg0: buf_ptr
/// arg1: len
/// 戻り値: (sender_id << 32) | received_len
pub fn recv(buf_ptr: u64, max_len: u64) -> u64 {
    let receiver = match crate::task::current_thread_id() {
        Some(id) => id.as_u64(),
        None => return EINVAL,
    };

    let (idx, receiver_generation) =
        match crate::task::thread_slot_index_and_generation_by_u64(receiver) {
            Some(v) => v,
            None => return EINVAL,
        };

    if idx >= MAX_THREADS || idx > (u16::MAX as usize) {
        return EINVAL;
    }

    let mut boxes = MAILBOXES.lock();
    let msg = loop {
        match boxes[idx].pop() {
            Some(msg)
                if msg.to == receiver
                    && msg.to_slot == idx as u16
                    && msg.to_generation == receiver_generation =>
            {
                break msg
            }
            Some(_) => continue, // 既に終了した別スレッド宛の古いメッセージを破棄
            None => return EAGAIN,
        }
    };
    drop(boxes); // ロック解除

    let copy_len = core::cmp::min(msg.len, max_len as usize);
    if copy_len > 0 && buf_ptr != 0 {
        // ユーザー空間アドレスの有効性を検証する
        if !crate::syscall::validate_user_ptr(buf_ptr, copy_len as u64) {
            return EFAULT;
        }
        crate::syscall::with_user_memory_access(|| unsafe {
            let dest_slice = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, copy_len);
            dest_slice.copy_from_slice(&msg.data[..copy_len]);
        });
    }

    // 上位32bitに送信元ID、下位32bitに長さ
    (msg.from << 32) | (copy_len as u64)
}

/// IPC受信（ブロッキング版）
/// メッセージが届くまでスレッドをスリープして待機する。
/// arg0: buf_ptr
/// arg1: len
pub fn recv_blocking(buf_ptr: u64, max_len: u64) -> u64 {
    let receiver = match crate::task::current_thread_id() {
        Some(id) => id,
        None => return EINVAL,
    };
    let receiver_u64 = receiver.as_u64();

    let (idx, receiver_generation) =
        match crate::task::thread_slot_index_and_generation_by_u64(receiver_u64) {
            Some(v) => v,
            None => return EINVAL,
        };

    if idx >= MAX_THREADS || idx > (u16::MAX as usize) {
        return EINVAL;
    }

    loop {
        // ロックを取得してメッセージを取り出すか、自分を waiter として登録する
        let msg = {
            let mut boxes = MAILBOXES.lock();
            // 有効なメッセージが来るまでキューを消化
            loop {
                match boxes[idx].pop() {
                    Some(msg)
                        if msg.to == receiver_u64
                            && msg.to_slot == idx as u16
                            && msg.to_generation == receiver_generation =>
                    {
                        break Some(msg);
                    }
                    Some(_) => continue, // 古いメッセージは捨てる
                    None => {
                        // メッセージなし：waiter として自分を登録してからロック解放
                        boxes[idx].waiter = receiver_u64;
                        break None;
                    }
                }
            }
        };

        match msg {
            Some(msg) => {
                let copy_len = core::cmp::min(msg.len, max_len as usize);
                if copy_len > 0 && buf_ptr != 0 {
                    if !crate::syscall::validate_user_ptr(buf_ptr, copy_len as u64) {
                        return EFAULT;
                    }
                    crate::syscall::with_user_memory_access(|| unsafe {
                        let dest_slice =
                            core::slice::from_raw_parts_mut(buf_ptr as *mut u8, copy_len);
                        dest_slice.copy_from_slice(&msg.data[..copy_len]);
                    });
                }
                return (msg.from << 32) | (copy_len as u64);
            }
            None => {
                // メッセージなし：pending_wakeup がなければスリープして yield
                if crate::task::sleep_thread_unless_woken(receiver) {
                    crate::task::yield_now();
                    // 実際にスリープして起床 → ループしてメッセージを再確認
                } else {
                    // pending_wakeup で即起床（子プロセス終了通知など）だがメッセージなし
                    // → waiter をクリアして 0 を返し、呼び出し元が終了検知できるようにする
                    {
                        let mut boxes = MAILBOXES.lock();
                        if boxes[idx].waiter == receiver_u64 {
                            boxes[idx].waiter = 0;
                        }
                    }
                    return 0;
                }
            }
        }
    }
}
