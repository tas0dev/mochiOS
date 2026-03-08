//! タスク管理モジュール
//!
//! マルチタスク機能を提供（プロセスとスレッドの管理）

pub mod context;
pub mod ids;
pub mod process;
pub mod scheduler;
pub mod thread;
pub mod usermode;
mod elf;

pub use context::{switch_context, switch_to_thread, Context};
pub use ids::{PrivilegeLevel, ProcessId, ProcessState, ThreadId, ThreadState};
pub use process::{
    add_process, find_process_id_by_name, for_each_process, has_child_process, mark_process_exited,
    process_count, reap_zombie_child_process, remove_process, with_process, with_process_mut,
    Process, ProcessTable,
};
pub use scheduler::{
    block_current_thread, disable_scheduler, enable_scheduler, exit_current_task, init_scheduler,
    is_scheduler_enabled, schedule, schedule_and_switch, scheduler_tick, set_time_slice,
    sleep_thread, start_scheduling, terminate_thread, wake_thread, yield_now, Scheduler,
};
pub use thread::{
    add_thread, count_threads_by_state, current_thread_id, for_each_thread, peek_next_thread,
    remove_thread, set_current_thread, thread_count, thread_id_exists, thread_slot_index,
    thread_slot_index_and_generation, thread_slot_index_and_generation_by_u64,
    thread_slot_index_by_u64, with_thread, with_thread_mut, Thread, ThreadQueue,
};
pub use usermode::{jump_to_usermode, jump_to_usermode_fork_child};
