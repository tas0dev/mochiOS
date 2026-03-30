use crate::interrupt::spinlock::SpinLock;

use super::{EAGAIN, EFAULT, EINVAL};

const MAX_THREADS: usize = crate::task::ThreadQueue::MAX_THREADS;
const MAILBOX_CAP: usize = 64;
const MAX_MSG_SIZE: usize = 2064; // DiskBulkResponse(2064) を収容

#[derive(Debug, Clone, Copy)]
pub struct Message {
    from: u64,
    to: u64,
    to_slot: u16,
    to_generation: u64,
    len: usize,
    data: [u8; MAX_MSG_SIZE],
    // Support up to 128 external pages (adjustable). Each entry is a physical page frame address.
    ext_pages_count: u16,
    ext_pages: [u64; 128],
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
            ext_pages_count: 0,
            ext_pages: [0; 128],
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Mailbox {
    head: usize,
    tail: usize,
    count: usize,
    queue: [u8; MAILBOX_CAP],
    slots: [Message; MAILBOX_CAP],
    free: [u8; MAILBOX_CAP],
    free_count: usize,
    /// メッセージ待ちでスリープ中のスレッドID (0=なし)
    waiter: u64,
}

impl Mailbox {
    const fn new() -> Self {
        let mut free = [0u8; MAILBOX_CAP];
        let mut i = 0;
        while i < MAILBOX_CAP {
            free[i] = i as u8;
            i += 1;
        }
        Self {
            head: 0,
            tail: 0,
            count: 0,
            queue: [0; MAILBOX_CAP],
            slots: [Message::empty(); MAILBOX_CAP],
            free,
            free_count: MAILBOX_CAP,
            waiter: 0,
        }
    }

    fn alloc_slot(&mut self) -> Option<usize> {
        if self.free_count == 0 {
            return None;
        }
        self.free_count -= 1;
        Some(self.free[self.free_count] as usize)
    }

    fn free_slot(&mut self, idx: usize) {
        self.free[self.free_count] = idx as u8;
        self.free_count += 1;
    }

    fn enqueue_slot(&mut self, slot_idx: usize) -> Result<(), ()> {
        if self.count >= MAILBOX_CAP {
            return Err(());
        }
        self.queue[self.tail] = slot_idx as u8;
        self.tail = (self.tail + 1) % MAILBOX_CAP;
        self.count += 1;
        Ok(())
    }

    fn dequeue_slot(&mut self) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let idx = self.queue[self.head] as usize;
        self.head = (self.head + 1) % MAILBOX_CAP;
        self.count -= 1;
        Some(idx)
    }

    fn push_message(
        &mut self,
        from: u64,
        to: u64,
        to_slot: u16,
        to_generation: u64,
        data: &[u8],
    ) -> Result<(), ()> {
        let slot_idx = match self.alloc_slot() {
            Some(i) => i,
            None => return Err(()),
        };
        let msg = &mut self.slots[slot_idx];
        msg.from = from;
        msg.to = to;
        msg.to_slot = to_slot;
        msg.to_generation = to_generation;
        msg.len = data.len();
        msg.ext_pages_count = 0;
        if !data.is_empty() {
            msg.data[..data.len()].copy_from_slice(data);
        }
        if self.enqueue_slot(slot_idx).is_err() {
            self.free_slot(slot_idx);
            return Err(());
        }
        Ok(())
    }

    fn pop_valid_for_receiver_copy(
        &mut self,
        receiver: u64,
        receiver_slot: u16,
        receiver_generation: u64,
        out: &mut [u8],
    ) -> Option<(u64, usize)> {
        while let Some(slot_idx) = self.dequeue_slot() {
            let msg = &self.slots[slot_idx];
            if msg.to == receiver
                && msg.to_slot == receiver_slot
                && msg.to_generation == receiver_generation
            {
                // If this message carries external pages and caller requested 0-copy mapping,
                // indicate that by returning len==0 and the receiver can inspect ext_pages_count/ext_pages
                let copy_len = core::cmp::min(msg.len, out.len());
                if msg.ext_pages_count > 0 && msg.len == 0 {
                    // leave out untouched; return 0 to indicate special pages-only message
                    let from = msg.from;
                    self.free_slot(slot_idx);
                    return Some((from, 0usize));
                }
                if copy_len > 0 {
                    out[..copy_len].copy_from_slice(&msg.data[..copy_len]);
                }
                let from = msg.from;
                self.free_slot(slot_idx);
                return Some((from, copy_len));
            }
            // 古い宛先のメッセージは破棄
            self.free_slot(slot_idx);
        }
        None
    }

    /// 指定送信元からの有効メッセージを1件だけ取り出し、内容を out へコピーする
    fn pop_from_sender_copy(
        &mut self,
        sender: u64,
        receiver: u64,
        receiver_slot: u16,
        receiver_generation: u64,
        out: &mut [u8],
    ) -> Option<(u64, usize)> {
        if self.count == 0 {
            return None;
        }

        let original = self.count;
        for _ in 0..original {
            let slot_idx = self.dequeue_slot()?;
            let msg = &self.slots[slot_idx];
            if msg.from != sender
                || msg.to != receiver
                || msg.to_slot != receiver_slot
                || msg.to_generation != receiver_generation
            {
                let _ = self.enqueue_slot(slot_idx);
                continue;
            }

            let copy_len = core::cmp::min(msg.len, out.len());
            if copy_len > 0 {
                out[..copy_len].copy_from_slice(&msg.data[..copy_len]);
            }
            let from = msg.from;
            self.free_slot(slot_idx);
            return Some((from, copy_len));
        }

        None
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
    let sender = crate::task::current_thread_id().map(|t| t.as_u64()).unwrap_or(0);
    MAILBOXES.lock().get_mut(idx).map_or(false, |mb| {
        if mb
            .push_message(sender, dest_thread_id, idx as u16, dest_generation, data)
            .is_ok()
        {
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

/// Kernel -> recipient: send a message that carries physical page frame addresses
/// Pages are explicit physical frame addresses (one per 4KiB page). Up to 32 entries supported.
pub fn send_pages_from_kernel(dest_thread_id: u64, map_start: u64, total: u64, pages: &[u64]) -> bool {
    if pages.len() > 128 {
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
    let sender = crate::task::current_thread_id().map(|t| t.as_u64()).unwrap_or(0);
    let mut boxes = MAILBOXES.lock();
    boxes.get_mut(idx).map_or(false, |mb| {
        if let Some(slot_idx) = mb.alloc_slot() {
            let msg = &mut mb.slots[slot_idx];
            msg.from = sender;
            msg.to = dest_thread_id;
            msg.to_slot = idx as u16;
            msg.to_generation = dest_generation;
            // serialize map_start, total, then pages into data
            let mut off = 0usize;
            if (16 + pages.len() * 8) > MAX_MSG_SIZE {
                mb.free_slot(slot_idx);
                return false;
            }
            msg.data[off..off + 8].copy_from_slice(&map_start.to_ne_bytes());
            off += 8;
            msg.data[off..off + 8].copy_from_slice(&(total as u64).to_ne_bytes());
            off += 8;
            for p in pages.iter() {
                msg.data[off..off + 8].copy_from_slice(&p.to_ne_bytes());
                off += 8;
            }
            msg.len = off;
            msg.ext_pages_count = pages.len() as u16;
            for i in 0..pages.len() {
                msg.ext_pages[i] = pages[i];
            }
            // enqueue
            if mb.enqueue_slot(slot_idx).is_err() {
                mb.free_slot(slot_idx);
                return false;
            }
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

    let mut boxes = MAILBOXES.lock();
    if boxes[idx]
        .push_message(sender, dest_thread_id, idx as u16, dest_generation, &data[..len])
        .is_err()
    {
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

    let max_copy = core::cmp::min(max_len as usize, MAX_MSG_SIZE);
    let mut recv_buf = [0u8; MAX_MSG_SIZE];
    let (from, copy_len) = {
        let mut boxes = MAILBOXES.lock();
        match boxes[idx].pop_valid_for_receiver_copy(
            receiver,
            idx as u16,
            receiver_generation,
            &mut recv_buf[..max_copy],
        ) {
            Some(v) => v,
            None => return EAGAIN,
        }
    };

    if copy_len > 0 && buf_ptr != 0 {
        // ユーザー空間アドレスの有効性を検証する
        if !crate::syscall::validate_user_ptr(buf_ptr, copy_len as u64) {
            return EFAULT;
        }
        crate::syscall::with_user_memory_access(|| unsafe {
            let dest_slice = core::slice::from_raw_parts_mut(buf_ptr as *mut u8, copy_len);
            dest_slice.copy_from_slice(&recv_buf[..copy_len]);
        });
    }

    // 上位32bitに送信元ID、下位32bitに長さ
    (from << 32) | (copy_len as u64)
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
        let max_copy = core::cmp::min(max_len as usize, MAX_MSG_SIZE);
        // ロックを取得してメッセージを取り出すか、自分を waiter として登録する
        let mut recv_buf = [0u8; MAX_MSG_SIZE];
        let recv = {
            let mut boxes = MAILBOXES.lock();
            match boxes[idx].pop_valid_for_receiver_copy(
                receiver_u64,
                idx as u16,
                receiver_generation,
                &mut recv_buf[..max_copy],
            ) {
                Some(v) => Some(v),
                None => {
                    // メッセージなし：waiter として自分を登録してからロック解放
                    boxes[idx].waiter = receiver_u64;
                    None
                }
            }
        };

        match recv {
            Some((from, copy_len)) => {
                if copy_len > 0 && buf_ptr != 0 {
                    if !crate::syscall::validate_user_ptr(buf_ptr, copy_len as u64) {
                        return EFAULT;
                    }
                    crate::syscall::with_user_memory_access(|| unsafe {
                        let dest_slice =
                            core::slice::from_raw_parts_mut(buf_ptr as *mut u8, copy_len);
                        dest_slice.copy_from_slice(&recv_buf[..copy_len]);
                    });
                }
                return (from << 32) | (copy_len as u64);
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

/// カーネル内部から、特定送信元のIPCをブロッキング受信する
///
/// - 受信データは `buf` へコピーされる（ユーザー空間検証は行わない）
/// - 指定送信元以外のメッセージはキューに保持されたまま
pub fn recv_blocking_from_sender_for_kernel(
    sender_thread_id: u64,
    buf: &mut [u8],
) -> Result<usize, u64> {
    let receiver = match crate::task::current_thread_id() {
        Some(id) => id,
        None => return Err(EINVAL),
    };
    let receiver_u64 = receiver.as_u64();

    let (idx, receiver_generation) =
        match crate::task::thread_slot_index_and_generation_by_u64(receiver_u64) {
            Some(v) => v,
            None => return Err(EINVAL),
        };
    if idx >= MAX_THREADS || idx > (u16::MAX as usize) {
        return Err(EINVAL);
    }

    loop {
        let n = {
            let mut boxes = MAILBOXES.lock();
            match boxes[idx].pop_from_sender_copy(
                sender_thread_id,
                receiver_u64,
                idx as u16,
                receiver_generation,
                buf,
            ) {
                Some((_, n)) => Some(n),
                None => {
                    boxes[idx].waiter = receiver_u64;
                    None
                }
            }
        };

        match n {
            Some(n) => return Ok(n),
            None => {
                if crate::task::sleep_thread_unless_woken(receiver) {
                    crate::task::yield_now();
                } else {
                    let mut boxes = MAILBOXES.lock();
                    if boxes[idx].waiter == receiver_u64 {
                        boxes[idx].waiter = 0;
                    }
                    return Err(EAGAIN);
                }
            }
        }
    }
}
