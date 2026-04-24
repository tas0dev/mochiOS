//! タイマー割込み管理
//!
//! PIT (Programmable Interval Timer) の管理とタイマー割込みハンドラ

use crate::debug;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::structures::idt::InterruptStackFrame;

/// タイマー割り込みカウンタ（100回 = 1秒）
static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);

/// タイマー割り込みハンドラ（IRQ0）
///
/// ## Arguments
/// - `_stack_frame`: 割り込み発生時のスタックフレーム
pub extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let entered_from_user = crate::syscall::syscall_entry::kpti_enter_for_trap(
        _stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3,
    );

    // タイマーカウンタを増加
    let ticks = TIMER_TICKS
        .fetch_add(1, Ordering::Relaxed)
        .saturating_add(1);
    crate::syscall::time::wake_due_sleepers(ticks);
    crate::syscall::process::wake_due_futex_waiters(ticks);

    // スケジューラのティックを実行
    let should_schedule = crate::task::scheduler_tick();

    // End of Interrupt (EOI) 信号をPICに送信
    super::send_eoi(32);

    // タイムスライスが尽きた場合はプリエンプト
    // switch_context がカーネルスタック状態を保存するため、
    // タイマーハンドラの iretq で自動的にユーザー/カーネルモードに戻る
    if should_schedule {
        crate::task::schedule_and_switch();
    }

    // ユーザーから入ってきた場合は、復帰先スレッドに応じたユーザーCR3へ戻す
    crate::syscall::syscall_entry::kpti_leave_after_trap(entered_from_user);
}

/// 現在のタイマーティック数を取得
///
/// ## Returns
/// - タイマーティック数（100回 = 1秒）
pub fn get_ticks() -> u64 {
    TIMER_TICKS.load(Ordering::Relaxed)
}

/// タイマーカウンタをリセット
pub fn reset_ticks() {
    TIMER_TICKS.store(0, Ordering::Relaxed);
}

/// PITを停止（UEFI起動時の状態をクリア）
pub fn disable_pit() {
    debug!("Disabling PIT...");
    unsafe {
        use x86_64::instructions::port::Port;

        // Channel 0を停止（one-shot mode、カウント0）
        Port::<u8>::new(0x43).write(0x30);
        Port::<u8>::new(0x40).write(0x00);
        Port::<u8>::new(0x40).write(0x00);
        // Channel 1,2も停止
        Port::<u8>::new(0x43).write(0x70); // Channel 1
        Port::<u8>::new(0x41).write(0x00);
        Port::<u8>::new(0x41).write(0x00);

        Port::<u8>::new(0x43).write(0xb0); // Channel 2
        Port::<u8>::new(0x42).write(0x00);
        Port::<u8>::new(0x42).write(0x00);
    }
    debug!("PIT disabled");
}

/// PITを初期化して10ms周期のタイマー割り込みを設定
pub fn init_pit() {
    debug!("Initializing PIT for 10ms timer interrupt...");
    unsafe {
        use x86_64::instructions::port::Port;

        // PIT base frequency: 1.193182 MHz
        // 10ms = 100 Hz
        // Divisor = 1193182 / 100 = 11932 (0x2E9C)
        let divisor: u16 = 11932;

        // Channel 0, LSB+MSB, Mode 2 (rate generator), Binary
        Port::<u8>::new(0x43).write(0x34);

        // IO待機
        for _ in 0..100 {
            core::hint::spin_loop();
        }

        // LSBを送信
        Port::<u8>::new(0x40).write((divisor & 0xff) as u8);

        // IO待機
        for _ in 0..100 {
            core::hint::spin_loop();
        }

        // MSBを送信
        Port::<u8>::new(0x40).write(((divisor >> 8) & 0xff) as u8);
    }
    debug!("PIT configured for 10ms interrupts");
}

/// タイマー割り込み（IRQ0）を有効化
pub fn enable_timer_interrupt() {
    debug!("Enabling timer interrupt (IRQ0)...");
    unsafe {
        use x86_64::instructions::port::Port;

        // Master: IRQ0(timer), IRQ1(keyboard), IRQ2(cascade) を許可
        // 1111_1000 = 0xF8
        Port::<u8>::new(0x21).write(0xf8);
        // Slave: IRQ12(PS/2 mouse) を許可（スレーブ内ではIRQ4）
        // 1110_1111 = 0xEF
        Port::<u8>::new(0xa1).write(0xef);

        // IO待機
        for _ in 0..1000 {
            core::hint::spin_loop();
        }
    }
    debug!("Timer interrupt enabled");
}
