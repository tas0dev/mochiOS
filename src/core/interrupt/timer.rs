//! タイマー割込み管理
//!
//! PIT (Programmable Interval Timer) の管理とタイマー割込みハンドラ

use crate::debug;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::structures::idt::InterruptStackFrame;

/// タイマー割り込みカウンタ（100回 = 1秒）
static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);

/// タイマー割り込みハンドラ（IRQ0）
pub extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // タイマーカウンタを増加
    let _ticks = TIMER_TICKS.fetch_add(1, Ordering::Relaxed);
    crate::debug!("timer_interrupt_handler: tick={}", _ticks + 1);

    // スケジューラのティックを実行
    // タイムスライスが尽きた場合はプリエンプトを行う
    let should_schedule = crate::task::scheduler_tick();

    // End of Interrupt (EOI) 信号をPICに送信
    super::send_eoi(32);

    // タイムスライスが尽きた場合はプリエンプト
    if should_schedule {
        crate::info!("timer: should_schedule = true");
        if let Some(next_id) = crate::task::schedule() {
            crate::info!("timer: schedule() -> next={:?}", next_id);
            let current = crate::task::current_thread_id();

            crate::debug!("timer: current={:?}", current);

            if Some(next_id) != current {
                crate::info!(
                    "timer: switching current != next, preparing saved context (next={:?})",
                    next_id
                );
                // 割り込み時点の RIP を取得
                let rip = _stack_frame.instruction_pointer.as_u64();

                // 現在の汎用レジスタとスタックポインタ、RFLAGS を収集
                let mut saved = crate::task::Context::new();

                let mut rbx_val: u64 = 0;
                let mut r12_val: u64 = 0;
                let mut r13_val: u64 = 0;
                let mut r14_val: u64 = 0;
                let mut r15_val: u64 = 0;
                let mut rbp_val: u64 = 0;
                let mut rsp_val: u64 = 0;
                let mut rflags_val: u64 = 0;

                unsafe {
                    core::arch::asm!(
                        "mov {rbx}, rbx",
                        "mov {r12}, r12",
                        "mov {r13}, r13",
                        "mov {r14}, r14",
                        "mov {r15}, r15",
                        "mov {rbp}, rbp",
                        "mov {rsp}, rsp",
                        "pushfq",
                        "pop {rflags}",
                        rbx = out(reg) rbx_val,
                        r12 = out(reg) r12_val,
                        r13 = out(reg) r13_val,
                        r14 = out(reg) r14_val,
                        r15 = out(reg) r15_val,
                        rbp = out(reg) rbp_val,
                        rsp = out(reg) rsp_val,
                        rflags = out(reg) rflags_val,
                    );
                }

                saved.rbx = rbx_val;
                saved.r12 = r12_val;
                saved.r13 = r13_val;
                saved.r14 = r14_val;
                saved.r15 = r15_val;
                saved.rbp = rbp_val;
                // Use the interrupt frame's saved stack pointer as the interrupted thread's RSP
                saved.rsp = _stack_frame.stack_pointer.as_u64();
                saved.rflags = rflags_val;

                saved.rip = rip;

                crate::debug!(
                    "timer: saved context: rsp={:#x}, rip={:#x}, rflags={:#x}",
                    saved.rsp,
                    saved.rip,
                    saved.rflags
                );

                // 次スレッドをCURRENTに設定してスイッチ実行
                crate::task::set_current_thread(Some(next_id));
                crate::info!("timer: set_current_thread({:?})", next_id);
                unsafe {
                    crate::task::context::switch_to_thread_from_isr(current, next_id, saved);
                }
            }
        }
    }
}

/// 現在のタイマーティック数を取得
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

        // PIC master のIRQ0とIRQ1のマスクを解除（ビット0/1を0にする）
        // タイマ（IRQ0）とキーボード（IRQ1）を許可するため 0b11111100 (0xfc)
        Port::<u8>::new(0x21).write(0xfc);

        // IO待機
        for _ in 0..1000 {
            core::hint::spin_loop();
        }
    }
    debug!("Timer interrupt enabled");
}
