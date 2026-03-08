//! PIC (Programmable Interrupt Controller) 管理

use crate::debug;

/// マスタPIC
pub struct Pic {
    /// 割り込みベクタオフセット
    offset: u8,
    /// コマンドポート
    command: u16,
    /// データポート
    data: u16,
}

impl Pic {
    /// End of Interrupt信号を送信
    ///
    /// # Safety
    /// 呼び出し側は、PICポートアクセスが現在の実行環境で有効であることを保証する必要がある。
    pub unsafe fn end_of_interrupt(&self) {
        use x86_64::instructions::port::Port;
        Port::new(self.command).write(0x20u8);
    }
}

/// マスタPICとスレーブPICの定義
pub const PIC_MASTER: Pic = Pic {
    /// 割込みベクタオフセット
    offset: 32,
    /// コマンドポート
    command: 0x20,
    /// データポート
    data: 0x21,
};

/// スレーブPIC
pub const PIC_SLAVE: Pic = Pic {
    /// 割込みベクタオフセット
    offset: 40,
    /// コマンドポート
    command: 0xa0,
    /// データポート
    data: 0xa1,
};

/// PICを初期化
pub fn init() {
    debug!("Initializing PIC...");

    unsafe {
        use x86_64::instructions::port::Port;

        // 先にすべての割り込みをマスク
        Port::<u8>::new(PIC_MASTER.data).write(0xffu8);
        Port::<u8>::new(PIC_SLAVE.data).write(0xffu8);
        for _ in 0..1000 {
            core::hint::spin_loop();
        }

        // ICW1: Initialize
        Port::new(PIC_MASTER.command).write(0x11u8);
        for _ in 0..100 {
            core::hint::spin_loop();
        }
        Port::new(PIC_SLAVE.command).write(0x11u8);
        for _ in 0..100 {
            core::hint::spin_loop();
        }

        // ICW2: Vector offset
        Port::new(PIC_MASTER.data).write(PIC_MASTER.offset);
        for _ in 0..100 {
            core::hint::spin_loop();
        }
        Port::new(PIC_SLAVE.data).write(PIC_SLAVE.offset);
        for _ in 0..100 {
            core::hint::spin_loop();
        }

        // ICW3: Cascade
        Port::new(PIC_MASTER.data).write(4u8); // Slave on IRQ2
        for _ in 0..100 {
            core::hint::spin_loop();
        }
        Port::new(PIC_SLAVE.data).write(2u8); // Cascade identity
        for _ in 0..100 {
            core::hint::spin_loop();
        }

        // ICW4: 8086 mode
        Port::new(PIC_MASTER.data).write(0x01u8);
        for _ in 0..100 {
            core::hint::spin_loop();
        }
        Port::new(PIC_SLAVE.data).write(0x01u8);
        for _ in 0..100 {
            core::hint::spin_loop();
        }

        // 再度すべての割り込みをマスク（念のため）
        Port::<u8>::new(PIC_MASTER.data).write(0xffu8);
        for _ in 0..100 {
            core::hint::spin_loop();
        }
        Port::<u8>::new(PIC_SLAVE.data).write(0xffu8);
        for _ in 0..100 {
            core::hint::spin_loop();
        }

        // EOIを送信して保留中の割り込みをクリア
        Port::<u8>::new(PIC_MASTER.command).write(0x20u8);
        Port::<u8>::new(PIC_SLAVE.command).write(0x20u8);
    }

    debug!("PIC initialized, all interrupts masked");
}

/// PICにEnd of Interrupt信号を送信
pub fn send_eoi(interrupt_id: u8) {
    unsafe {
        if interrupt_id >= 40 {
            // Slave PIC
            PIC_SLAVE.end_of_interrupt();
        }
        // Master PIC
        PIC_MASTER.end_of_interrupt();
    }
}
