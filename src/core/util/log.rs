//! ロギングユーティリティ

use core::sync::atomic::{AtomicU8, Ordering};

/// ログレベル
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// 現在のログレベル（デフォルト: Info）
static LOG_LEVEL: AtomicU8 = AtomicU8::new(2);

fn level_to_u8(level: LogLevel) -> u8 {
    match level {
        LogLevel::Trace => 0,
        LogLevel::Debug => 1,
        LogLevel::Info => 2,
        LogLevel::Warn => 3,
        LogLevel::Error => 4,
    }
}

fn should_log(level: LogLevel) -> bool {
    level_to_u8(level) >= LOG_LEVEL.load(Ordering::Relaxed)
}

/// ログレベルを設定
pub fn set_level(level: LogLevel) {
    LOG_LEVEL.store(level_to_u8(level), Ordering::Relaxed);
}

/// ログ出力（シリアルとVGAの両方）
pub fn log(level: LogLevel, args: core::fmt::Arguments) {
    if !should_log(level) {
        return;
    }

    use crate::{sprint, sprintln, vprint, vprintln};

    let prefix = match level {
        LogLevel::Trace => "[TRACE]",
        LogLevel::Debug => "[DEBUG]",
        LogLevel::Info => "[INFO] ",
        LogLevel::Warn => "[WARN] ",
        LogLevel::Error => "[ERROR]",
    };

    sprint!("{} ", prefix);
    sprintln!("{}", args);
}

/// トレースログ
#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        $crate::util::log::log($crate::util::log::LogLevel::Trace, format_args!($($arg)*))
    };
}

/// デバッグログ
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        $crate::util::log::log($crate::util::log::LogLevel::Debug, format_args!($($arg)*))
    };
}

/// 情報ログ
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::util::log::log($crate::util::log::LogLevel::Info, format_args!($($arg)*))
    };
}

/// 警告ログ
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::util::log::log($crate::util::log::LogLevel::Warn, format_args!($($arg)*))
    };
}

/// エラーログ
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::util::log::log($crate::util::log::LogLevel::Error, format_args!($($arg)*))
    };
}
