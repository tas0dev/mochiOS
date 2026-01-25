//! ユーザー側システムコールスタブ
//!
//! 種類ごとのモジュールに分割し、ここから再エクスポートする。

pub mod ipc;
pub mod task;
pub mod time;
pub mod console;
pub mod fs;
pub mod keyboard;

mod sys;

pub use sys::SyscallNumber;
pub use ipc::{ipc_recv, ipc_send};
pub use task::{yield_now, exit, current_thread_id, thread_id_by_name};
pub use time::get_ticks;
pub use console::write as console_write;
pub use fs::read as initfs_read;
pub use keyboard::read_char as keyboard_read_char;
