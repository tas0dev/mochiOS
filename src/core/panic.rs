//! パニックハンドラ
//!
//! カーネルパニック時の処理
//! 通常のパニックは最終手段として使用し、可能な限りResult型でエラー処理を行うこと

use crate::{result, warn};

/// エラーコンテキスト（パニック時に使用）
pub struct ErrorContext {
    /// エラーメッセージ
    pub error: &'static str,
    /// 発生ファイル
    pub file: &'static str,
    /// 発生行
    pub line: u32,
    /// 発生列
    pub column: u32,
}

#[allow(deprecated)]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    crate::info!("!!! KERNEL PANIC !!!");

    if let Some(loc) = info.location() {
        warn!("Location: {}:{}:{}", loc.file(), loc.line(), loc.column());
    }

    if let Some(msg) = info.message().as_str() {
        warn!("Message: {}", msg);
    } else if let Some(s) = info.payload().downcast_ref::<&str>() {
        warn!("Message: {}", s);
    }

    warn!("System halted. Please reset.");

    // 割り込みを無効化
    #[cfg(target_arch = "x86_64")]
    unsafe {
        x86_64::instructions::interrupts::disable();
    }

    // システムを停止
    loop {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

/// エラーコンテキスト付きのパニックマクロ（開発用）
///
/// 本番環境ではResult型を使用し、このマクロは使用しないこと
#[macro_export]
macro_rules! kernel_panic {
    ($msg:expr) => {
        {
            $crate::warn!("[KERNEL PANIC] {}", $msg);
            #[cfg(target_arch = "x86_64")]
            unsafe {
                x86_64::instructions::interrupts::disable();
            }
            loop {
                #[cfg(target_arch = "x86_64")]
                unsafe { core::arch::asm!("hlt"); }
            }
        }
    };
    ($fmt:expr, $($arg:tt)*) => {
        {
            $crate::warn!($fmt, $($arg)*);
            #[cfg(target_arch = "x86_64")]
            unsafe {
                x86_64::instructions::interrupts::disable();
            }
            loop {
                #[cfg(target_arch = "x86_64")]
                unsafe { core::arch::asm!("hlt"); }
            }
        }
    };
}
