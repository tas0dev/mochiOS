use crate::interrupt::spinlock::SpinLock;

use super::context::switch_to_thread;
use super::ids::{ThreadId, ThreadState};
use super::thread::{
    current_thread_id, remove_thread, set_current_thread, with_thread, with_thread_mut,
    THREAD_QUEUE,
};

/// スケジューラ
///
/// スレッドのスケジューリングを管理
pub struct Scheduler {
    /// スケジューラが有効かどうか
    enabled: bool,
    /// タイムスライス（タイマー割り込み回数）
    time_slice: u64,
    /// 現在のタイムスライスカウンタ
    current_slice: u64,
}

impl Scheduler {
    /// デフォルトのタイムスライス（10ms × 2 = 20ms）
    pub const DEFAULT_TIME_SLICE: u64 = 2;

    /// 新しいスケジューラを作成
    pub const fn new() -> Self {
        Self {
            enabled: false,
            time_slice: Self::DEFAULT_TIME_SLICE,
            current_slice: 0,
        }
    }

    /// スケジューラを有効化
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// スケジューラを無効化
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// スケジューラが有効かどうか
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// タイムスライスを設定
    pub fn set_time_slice(&mut self, slice: u64) {
        self.time_slice = slice;
    }

    /// タイマー割り込み時に呼ばれる
    ///
    /// タイムスライスをカウントし、期限が来たらスケジューリングを実行
    pub fn tick(&mut self) -> bool {
        if !self.enabled {
            return false;
        }

        self.current_slice += 1;
        if self.current_slice >= self.time_slice {
            self.current_slice = 0;
            true // スケジューリングが必要
        } else {
            false
        }
    }

    /// タイムスライスをリセット
    pub fn reset_slice(&mut self) {
        self.current_slice = 0;
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// グローバルスケジューラ
static SCHEDULER: SpinLock<Scheduler> = SpinLock::new(Scheduler::new());

/// スケジューラを初期化
pub fn init_scheduler() {
    let mut scheduler = SCHEDULER.lock();
    scheduler.enable();
}

/// スケジューラを有効化
pub fn enable_scheduler() {
    SCHEDULER.lock().enable();
}

/// タイムスライスを設定
pub fn set_time_slice(slice: u64) {
    SCHEDULER.lock().set_time_slice(slice);
}

/// スケジューラを無効化
pub fn disable_scheduler() {
    SCHEDULER.lock().disable();
}

/// スケジューラが有効かどうか
pub fn is_scheduler_enabled() -> bool {
    SCHEDULER.lock().is_enabled()
}

/// タイマー割り込み時に呼ばれる（タイマー割り込みハンドラから呼び出す）
///
/// # Returns
/// スケジューリングが必要な場合はtrue
pub fn scheduler_tick() -> bool {
    if let Some(tid) = current_thread_id() {
        if with_thread(tid, |t| t.in_syscall()).unwrap_or(false) {
            return false;
        }
    }
    SCHEDULER.lock().tick()
}

/// 次に実行すべきスレッドを選択
///
/// ラウンドロビンスケジューリング：Ready状態のスレッドを順に選択
///
/// # Returns
/// 次に実行すべきスレッドID。実行可能なスレッドがない場合はNone
pub fn schedule() -> Option<ThreadId> {
    let mut queue = THREAD_QUEUE.lock();

    // 現在のスレッドを取得
    let current = current_thread_id();

    // 現在のスレッドがあれば、状態をReadyに戻す（Running -> Ready）
    if let Some(current_id) = current {
        if let Some(thread) = queue.get_mut(current_id) {
            if thread.state() == ThreadState::Running {
                thread.set_state(ThreadState::Ready);
            }
        }
    }

    // 現在のスレッドの次のReady状態のスレッドを探す
    if let Some(next_thread) = queue.peek_next_after(current) {
        let next_id = next_thread.id();
        next_thread.set_state(ThreadState::Running);

        // スケジューラのタイムスライスをリセット
        drop(queue);
        SCHEDULER.lock().reset_slice();

        Some(next_id)
    } else {
        None
    }
}

/// 現在のスレッドを明示的にCPUを手放す（yield）
///
/// スケジューラを呼び出して次のスレッドに切り替える
pub fn yield_now() {
    if !is_scheduler_enabled() {
        return;
    }

    crate::debug!("yield_now() called");

    // スケジューリングと切り替えは割り込み禁止区間で実行し、
    // 状態更新と実際の切替の間に割り込みが入る競合窓を防ぐ。
    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(next_id) = schedule() {
            let current = current_thread_id();

            crate::debug!("yield_now: current={:?}, next={:?}", current, next_id);

            // 次のスレッドが現在のスレッドと異なる場合のみ切り替え
            if Some(next_id) != current {
                crate::debug!("Calling switch_to_thread...");

                // コンテキストスイッチを実行
                unsafe {
                    switch_to_thread(current, next_id);
                }

                crate::debug!("Returned from switch_to_thread");
            }
        }
    });
}

/// スレッドをブロック状態にする
///
/// 現在のスレッドをBlocked状態にして、次のスレッドにスケジューリング
pub fn block_current_thread() {
    if let Some(current_id) = current_thread_id() {
        with_thread_mut(current_id, |thread| {
            thread.set_state(ThreadState::Blocked);
        });

        // 次のスレッドにスケジューリング
        yield_now();
    }
}

/// スレッドをスリープ状態にする
///
/// 指定されたスレッドをSleeping状態にする
pub fn sleep_thread(id: ThreadId) {
    with_thread_mut(id, |thread| {
        thread.set_state(ThreadState::Sleeping);
    });
}

/// スレッドを起床させる
///
/// Sleeping/Blocked状態のスレッドをReady状態にする。
/// Ready状態の場合は pending_wakeup フラグを立てて競合を防ぐ。
pub fn wake_thread(id: ThreadId) {
    with_thread_mut(id, |thread| {
        let state = thread.state();
        if state == ThreadState::Sleeping || state == ThreadState::Blocked {
            thread.set_state(ThreadState::Ready);
        } else if state == ThreadState::Ready {
            // まだ眠っていない場合、起床要求を記録しておく
            thread.set_pending_wakeup();
        }
    });
}

/// 現在のスレッドをスリープ状態にする。
///
/// pending_wakeup フラグが立っていれば眠らずに即座に返す（競合回避）。
/// # Returns
/// `true` なら実際に Sleeping 状態に遷移した。`false` なら眠らなかった。
pub fn sleep_thread_unless_woken(id: ThreadId) -> bool {
    with_thread_mut(id, |thread| {
        if thread.take_pending_wakeup() {
            // 先に wake が呼ばれていたので眠らない
            false
        } else {
            thread.set_state(ThreadState::Sleeping);
            true
        }
    })
    .unwrap_or(false)
}

/// 子プロセス終了時に親プロセスの先頭スレッドの IPC waiter を起床させる。
/// IPC recv_blocking でスリープしている親スレッドを叩き起こし、child exit を検知させる。
fn wake_parent_ipc_waiter(exited_pid: crate::task::ProcessId) {
    use crate::task::with_process;
    let parent_pid = match with_process(exited_pid, |p| p.parent_id()) {
        Some(Some(pid)) => pid,
        _ => return,
    };

    // 親プロセスの最初のスレッドを探し、IPC mailbox に積まれた waiter を起床させる
    let mut parent_tid: Option<ThreadId> = None;
    crate::task::for_each_thread(|thread| {
        if parent_tid.is_none() && thread.process_id() == parent_pid {
            parent_tid = Some(thread.id());
        }
    });

    if let Some(tid) = parent_tid {
        // ゼロ長メッセージを mailbox に積んで recv_blocking が確実に戻れるようにする。
        // wake_thread だけでは「スリープ中に Ready に変えて pending_wakeup なし」の場合、
        // recv_blocking が yield 後に再スリープしてしまうため、必ずメッセージを使う。
        crate::syscall::ipc::send_from_kernel(tid.as_u64(), &[]);
    }
}

/// スレッドを終了させる
///
/// 指定されたスレッドをTerminated状態にして削除
pub fn terminate_thread(id: ThreadId) {
    with_thread_mut(id, |thread| {
        thread.set_state(ThreadState::Terminated);
    });

    // 現在のスレッドの場合は次のスレッドにスケジューリング
    if Some(id) == current_thread_id() {
        set_current_thread(None);
        yield_now();
    }

    crate::syscall::process::clear_futex_waiter(id);
    // スレッドをキューから削除し、カーネルスタックを解放
    if let Some(thread) = remove_thread(id) {
        crate::task::free_kernel_stack(thread.kernel_stack_base());
    }
}

/// 現在のタスクを終了させる（exitシステムコール用）
///
/// 現在のスレッドをTerminated状態にして削除し、次のスレッドにスケジューリング
pub fn exit_current_task(exit_code: u64) -> ! {
    if let Some(current_id) = current_thread_id() {
        crate::debug!("Exiting thread {:?} with code {}", current_id, exit_code);
        let current_pid = with_thread(current_id, |thread| thread.process_id());

        with_thread_mut(current_id, |thread| {
            thread.set_state(ThreadState::Terminated);
        });

        if let Some(pid) = current_pid {
            let mut has_other_live_threads = false;
            crate::task::for_each_thread(|thread| {
                if thread.process_id() == pid
                    && thread.id() != current_id
                    && thread.state() != ThreadState::Terminated
                {
                    has_other_live_threads = true;
                }
            });
            if !has_other_live_threads {
                crate::task::mark_process_exited(pid, exit_code);
                // 親プロセスが IPC でブロックしている可能性があるので起床させる
                wake_parent_ipc_waiter(pid);
                // 親プロセスへ SIGCHLD を送達する
                crate::syscall::signal::deliver_sigchld_to_parent(pid);
            }
        }

        // 現在のスレッドをクリア（先にクリアしないとschedule()が正しく動作しない）
        set_current_thread(None);

        x86_64::instructions::interrupts::without_interrupts(|| {
            // 次のスレッドにスケジューリング（戻ってこない）
            if let Some(next_id) = schedule() {
                crate::debug!("Switching from exited thread to {:?}", next_id);

                // スレッドをキューから削除（コンテキストスイッチ前に削除）
                crate::syscall::process::clear_futex_waiter(current_id);
                let kstack_base = with_thread(current_id, |t| t.kernel_stack_base()).unwrap_or(0);
                remove_thread(current_id);

                // カーネルスタックをフリーリストへ返却（スイッチ直前、まだスタックは有効）
                crate::task::free_kernel_stack(kstack_base);

                // コンテキストスイッチを実行（終了したスレッドのコンテキストは保存しない）
                // old_context_ptr = None を渡すことで、現在のコンテキストを保存せずに次のスレッドにジャンプ
                unsafe {
                    switch_to_thread(None, next_id);
                }

                crate::sprintln!("switch_to_thread returned unexpectedly; halting.");
                loop {
                    x86_64::instructions::hlt();
                }
            }
        });

        // スレッドをキューから削除
        crate::syscall::process::clear_futex_waiter(current_id);
        if let Some(thread) = remove_thread(current_id) {
            crate::task::free_kernel_stack(thread.kernel_stack_base());
        }
    }

    // スレッドがない場合は永久にhaltして待機
    crate::sprintln!("No more user threads. Halting system.");
    loop {
        x86_64::instructions::hlt();
    }
}

/// スケジューリングしてコンテキストスイッチを実行
///
/// タイマー割り込みハンドラから呼び出される
pub fn schedule_and_switch() {
    if !is_scheduler_enabled() {
        return;
    }

    x86_64::instructions::interrupts::without_interrupts(|| {
        let current = current_thread_id();

        // 次のスレッドを選択
        if let Some(next_id) = schedule() {
            // 次のスレッドが現在のスレッドと異なる場合のみ切り替え
            if Some(next_id) != current {
                // コンテキストスイッチを実行
                unsafe {
                    switch_to_thread(current, next_id);
                }
            }
        }
    });
}

/// 最初のスレッドを起動
///
/// スケジューラを開始して最初のスレッドにジャンプ
pub fn start_scheduling() -> ! {
    // 最初のスレッドを選択
    if let Some(first_id) = super::thread::peek_next_thread() {
        x86_64::instructions::interrupts::without_interrupts(|| {
            with_thread_mut(first_id, |thread| {
                crate::info!(
                    "Starting first thread: {} (id={:?})",
                    thread.name(),
                    thread.id()
                );
                thread.set_state(ThreadState::Running);
            });

            // 最初のスレッドへ switch_to_thread でジャンプ（戻ってこない）
            // user/kernel どちらも switch_context 経由で正しく動作する
            unsafe {
                switch_to_thread(None, first_id);
            }
        });

        crate::sprintln!("switch_to_thread returned unexpectedly; halting.");
        loop {
            x86_64::instructions::hlt();
        }
    } else {
        crate::sprintln!("No threads to schedule; halting system.");
        loop {
            x86_64::instructions::hlt();
        }
    }
}

/// プロセス終了用のエイリアス（ページフォルトハンドラなどから呼び出される）
///
/// 現在のプロセス/スレッドを終了させる
pub fn exit_current_process(exit_code: i32) -> ! {
    exit_current_task(exit_code as u64)
}
