//! PS/2 マウス入力
//!
//! IRQ12 から届くバイト列を 3 バイトのパケットへ組み立てて FIFO に積む。

use super::fifo::Fifo;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// 完成済みマウスパケットのキュー（b0 | b1<<8 | b2<<16）
pub static MOUSE_PACKET_BUF: Fifo<u32, 256> = Fifo::new();

/// read で待機しているスレッド（0 = 待機なし）
static MOUSE_WAITER: AtomicU64 = AtomicU64::new(0);
static MOUSE_PACKET_DROPS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
struct PacketAssembler {
    bytes: [u8; 3],
    len: u8,
}

static ASSEMBLER: Mutex<PacketAssembler> = Mutex::new(PacketAssembler {
    bytes: [0; 3],
    len: 0,
});

const STATUS_OUTPUT_FULL: u8 = 1 << 0;
const STATUS_INPUT_FULL: u8 = 1 << 1;

#[inline]
fn wait_input_clear(mut budget: usize) -> bool {
    use x86_64::instructions::port::Port;
    while budget > 0 {
        let status: u8 = unsafe { Port::<u8>::new(0x64).read() };
        if (status & STATUS_INPUT_FULL) == 0 {
            return true;
        }
        budget -= 1;
        core::hint::spin_loop();
    }
    false
}

#[inline]
fn wait_output_full(mut budget: usize) -> bool {
    use x86_64::instructions::port::Port;
    while budget > 0 {
        let status: u8 = unsafe { Port::<u8>::new(0x64).read() };
        if (status & STATUS_OUTPUT_FULL) != 0 {
            return true;
        }
        budget -= 1;
        core::hint::spin_loop();
    }
    false
}

#[inline]
fn read_data_with_timeout(budget: usize) -> Option<u8> {
    use x86_64::instructions::port::Port;
    if !wait_output_full(budget) {
        return None;
    }
    Some(unsafe { Port::<u8>::new(0x60).read() })
}

#[inline]
fn write_controller_command(cmd: u8) -> bool {
    use x86_64::instructions::port::Port;
    if !wait_input_clear(100_000) {
        return false;
    }
    unsafe {
        Port::<u8>::new(0x64).write(cmd);
    }
    true
}

#[inline]
fn write_controller_data(data: u8) -> bool {
    use x86_64::instructions::port::Port;
    if !wait_input_clear(100_000) {
        return false;
    }
    unsafe {
        Port::<u8>::new(0x60).write(data);
    }
    true
}

fn flush_output_buffer() {
    use x86_64::instructions::port::Port;
    for _ in 0..32 {
        if !wait_output_full(1000) {
            break;
        }
        let _: u8 = unsafe { Port::<u8>::new(0x60).read() };
    }
}

fn send_mouse_command(cmd: u8) -> bool {
    if !write_controller_command(0xD4) {
        return false;
    }
    if !write_controller_data(cmd) {
        return false;
    }
    matches!(read_data_with_timeout(100_000), Some(0xFA))
}

/// PS/2 マウスを初期化する。
///
/// 成功時 `true`、失敗時 `false`。
pub fn init() -> bool {
    flush_output_buffer();

    // AUX (mouse) ポート有効化
    if !write_controller_command(0xA8) {
        return false;
    }

    // コントローラ設定バイトを読み出し、IRQ12を有効化
    if !write_controller_command(0x20) {
        return false;
    }
    let Some(config) = read_data_with_timeout(100_000) else {
        return false;
    };
    // bit1: IRQ12 enable, bit5: mouse clock disable
    let updated = (config | 0x02) & !0x20;
    if !write_controller_command(0x60) {
        return false;
    }
    if !write_controller_data(updated) {
        return false;
    }

    // 既定値に戻し、データ報告を有効化
    if !send_mouse_command(0xF6) {
        return false;
    }
    if !send_mouse_command(0xF4) {
        return false;
    }

    true
}

/// IRQ12 から届いた 1 バイトを取り込み、3バイト揃ったらパケット化する
pub fn push_byte(byte: u8) {
    let mut assembler = ASSEMBLER.lock();

    // 先頭バイトは常に bit3=1。同期が崩れた場合はここで再同期する。
    if assembler.len == 0 && (byte & 0x08) == 0 {
        return;
    }

    let idx = assembler.len as usize;
    if idx >= assembler.bytes.len() {
        assembler.len = 0;
        return;
    }
    assembler.bytes[idx] = byte;
    assembler.len += 1;

    if assembler.len < 3 {
        return;
    }

    let b0 = assembler.bytes[0];
    let b1 = assembler.bytes[1];
    let b2 = assembler.bytes[2];
    assembler.len = 0;
    drop(assembler);

    // オーバーフローは破棄
    if (b0 & 0xC0) != 0 {
        return;
    }

    let packet = u32::from(b0) | (u32::from(b1) << 8) | (u32::from(b2) << 16);
    push_packet(packet);
}

/// 完成済みパケットを直接キューへ積む（ユーザー空間注入用）
pub fn push_packet(packet: u32) {
    if MOUSE_PACKET_BUF.push(packet).is_err() {
        MOUSE_PACKET_DROPS.fetch_add(1, Ordering::Relaxed);
    }

    let waiter = MOUSE_WAITER.swap(0, Ordering::AcqRel);
    if waiter != 0 {
        crate::task::wake_thread(crate::task::ThreadId::from_u64(waiter));
    }
}

pub fn packet_drop_count() -> u64 {
    MOUSE_PACKET_DROPS.load(Ordering::Relaxed)
}

/// 完成済みパケットを 1 つ取り出す
pub fn pop_packet() -> Option<u32> {
    MOUSE_PACKET_BUF.pop()
}

/// ブロッキング read 用に待機スレッドを登録する
pub fn register_waiter(tid: u64) -> bool {
    MOUSE_WAITER
        .compare_exchange(0, tid, Ordering::Release, Ordering::Acquire)
        .is_ok()
}

/// 待機登録をキャンセルする
pub fn unregister_waiter(tid: u64) {
    let _ = MOUSE_WAITER.compare_exchange(tid, 0, Ordering::AcqRel, Ordering::Relaxed);
}
