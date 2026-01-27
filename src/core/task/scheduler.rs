use crate::interrupt::spinlock::SpinLock;

use super::context::switch_to_thread;
use super::ids::{ThreadId, ThreadState};
use super::thread::{
    current_thread_id, remove_thread, set_current_thread, with_thread_mut, CURRENT_THREAD,
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
    /// デフォルトのタイムスライス（10ms × 10 = 100ms）
    pub const DEFAULT_TIME_SLICE: u64 = 10;

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
    let current = *CURRENT_THREAD.lock();

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

    // スケジューリングを実行
    if let Some(next_id) = schedule() {
        let current = current_thread_id();

        crate::debug!("yield_now: current={:?}, next={:?}", current, next_id);

        // 次のスレッドが現在のスレッドと異なる場合のみ切り替え
        if Some(next_id) != current {
            set_current_thread(Some(next_id));

            crate::debug!("Calling switch_to_thread...");

            // コンテキストスイッチを実行
            unsafe {
                switch_to_thread(current, next_id);
            }

            crate::debug!("Returned from switch_to_thread");
        }
    }
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
/// Sleeping/Blocked状態のスレッドをReady状態にする
pub fn wake_thread(id: ThreadId) {
    with_thread_mut(id, |thread| {
        let state = thread.state();
        if state == ThreadState::Sleeping || state == ThreadState::Blocked {
            thread.set_state(ThreadState::Ready);
        }
    });
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

    // スレッドをキューから削除
    remove_thread(id);
}

/// スケジューリングしてコンテキストスイッチを実行
///
/// タイマー割り込みハンドラから呼び出される
pub fn schedule_and_switch() {
    if !is_scheduler_enabled() {
        return;
    }

    let current = current_thread_id();

    // 次のスレッドを選択
    if let Some(next_id) = schedule() {
        // 次のスレッドが現在のスレッドと異なる場合のみ切り替え
        if Some(next_id) != current {
            set_current_thread(Some(next_id));

            // コンテキストスイッチを実行
            unsafe {
                switch_to_thread(current, next_id);
            }
        }
    }
}

/// 最初のスレッドを起動
///
/// スケジューラを開始して最初のスレッドにジャンプ
pub fn start_scheduling() -> ! {
    // 最初のスレッドを選択
    if let Some(first_id) = super::thread::peek_next_thread() {
        set_current_thread(Some(first_id));

        // 情報出力（表示確実化のため info にする）
        with_thread_mut(first_id, |thread| {
            crate::info!(
                "Starting first thread: {} (id={:?})",
                thread.name(),
                thread.id()
            );
            crate::info!(
                "  Context: rsp={:#x}, rip={:#x}, rflags={:#x}",
                thread.context().rsp,
                thread.context().rip,
                thread.context().rflags
            );
            thread.set_state(ThreadState::Running);
        });

        // 最初のスレッドにジャンプ（戻ってこない）
        // Determine if the first thread is user (Service) and capture its context pointer
        let (is_user, ctx_ptr) = crate::task::with_thread(first_id, |thread| {
            let priv_level = crate::task::with_process(thread.process_id(), |p| p.privilege())
                .unwrap_or(crate::task::PrivilegeLevel::Core);
            (priv_level != crate::task::PrivilegeLevel::Core, thread.context() as *const _)
        })
        .unwrap_or((false, core::ptr::null()));

        unsafe {
            if is_user {
                let ctx = &*ctx_ptr;
                crate::info!("Entering user mode for first thread via enter_user_from_kernel");
                crate::task::context::enter_user_from_kernel(ctx);
            } else {
                switch_to_thread(None, first_id);
            }
        }

        unreachable!("switch_to_thread should never return");
    } else {
        panic!("No threads to schedule!");
    }
}
