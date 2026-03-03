use crate::interrupt::spinlock::SpinLock;

use super::context::Context;
use super::ids::{ProcessId, ThreadId, ThreadState};

/// スレッド終了時に呼ばれるハンドラ
/// この関数から戻ることはない
extern "C" fn thread_exit_handler() -> ! {
    // スレッドが終了した場合の処理
    // 通常はここに到達することはない
    loop {
        x86_64::instructions::hlt();
    }
}

/// スレッド構造体
///
/// プロセス内で実行される軽量な実行単位。
/// 同じプロセス内のスレッドはメモリ空間を共有する。
pub struct Thread {
    /// スレッドID
    id: ThreadId,
    /// 所属するプロセスID
    process_id: ProcessId,
    /// スレッド名 (固定長バッファ)
    name: [u8; 32],
    /// 有効な名前の長さ
    name_len: usize,
    /// 現在の状態
    state: ThreadState,
    /// CPUコンテキスト
    context: Context,
    /// カーネルスタックの開始アドレス
    kernel_stack: u64,
    /// カーネルスタックのサイズ
    kernel_stack_size: usize,
    /// ユーザーモードエントリポイント（0の場合はカーネルモードスレッド）
    user_entry: u64,
    /// ユーザースタックトップ（0の場合はカーネルモードスレッド）
    user_stack: u64,
    /// fork時に子プロセスへ渡すユーザー RFLAGS
    fork_user_rflags: u64,
    /// TLS用 FS ベースレジスタ (arch_prctl ARCH_SET_FS で設定)
    fs_base: u64,
    /// 現在システムコールコンテキスト中かどうか
    in_syscall: bool,
    /// KPTI 復帰用のユーザーCR3
    syscall_user_cr3: u64,
}

// Simple kernel stack pool for creating kernel stacks for threads
const KSTACK_POOL_SIZE: usize = 4096 * 64; // 256 KiB
const KSTACK_GUARD_BYTES: usize = 4096;
static KSTACK_POOL: SpinLock<[u8; KSTACK_POOL_SIZE]> = SpinLock::new([0; KSTACK_POOL_SIZE]);
static NEXT_KSTACK_OFFSET: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// カーネルスタックを内部プールから割り当てます。
/// Returns base address (bottom) of stack.
pub fn allocate_kernel_stack(size: usize) -> Option<u64> {
    if size == 0 || size > KSTACK_POOL_SIZE.saturating_sub(KSTACK_GUARD_BYTES) {
        return None;
    }
    // align size to 16
    let size = (size + 0xF) & !0xF;
    let alloc_size = size.checked_add(KSTACK_GUARD_BYTES)?;
    let off = NEXT_KSTACK_OFFSET.fetch_add(alloc_size, core::sync::atomic::Ordering::SeqCst);
    if off + alloc_size > KSTACK_POOL_SIZE {
        return None;
    }
    // ガード領域を確保してから実スタックを返す（論理ガード）
    let pool = KSTACK_POOL.lock();
    let ptr = (pool.as_ptr() as usize + off + KSTACK_GUARD_BYTES) as u64;
    Some(ptr)
}

impl Thread {
    /// 新しいスレッドを作成
    ///
    /// # Arguments
    /// * `process_id` - 所属するプロセスID
    /// * `name` - スレッド名
    /// * `entry_point` - スレッドのエントリーポイント関数
    /// * `kernel_stack` - カーネルスタックのアドレス
    /// * `kernel_stack_size` - カーネルスタックのサイズ
    pub fn new(
        process_id: ProcessId,
        name: &str,
        entry_point: fn() -> !,
        kernel_stack: u64,
        kernel_stack_size: usize,
    ) -> Self {
        let mut name_buf = [0u8; 32];
        let bytes = name.as_bytes();
        let len = core::cmp::min(bytes.len(), 32);
        name_buf[..len].copy_from_slice(&bytes[..len]);

        let mut context = Context::new();

        // スタックポインタをスタックの最後に設定（スタックは下に伸びる）
        // 16バイト境界に合わせる
        let stack_top = (kernel_stack + kernel_stack_size as u64) & !0xF;

        // 呼び出し規約に合わせて、戻り先アドレス用のスロットを確保
        let stack_ptr = stack_top - 8;

        unsafe {
            // 戻り先として thread_exit_handler を配置
            let ret_addr = stack_ptr as *mut u64;
            *ret_addr = thread_exit_handler as *const () as u64;
        }

        // rsp は「戻り先アドレスが置かれている位置」を指す
        context.rsp = stack_ptr;
        context.rbp = stack_top;

        // エントリーポイントをripに設定
        context.rip = entry_point as u64;

        // RFLAGSの初期値（割り込み有効）
        context.rflags = 0x202; // IF (Interrupt Flag) = 1

        crate::debug!(
            "Creating thread '{}': stack={:#x}, size={:#x}, rsp={:#x}, rip={:#x}",
            name,
            kernel_stack,
            kernel_stack_size,
            context.rsp,
            context.rip
        );

        Self {
            id: ThreadId::new(),
            process_id,
            name: name_buf,
            name_len: len,
            state: ThreadState::Ready,
            context,
            kernel_stack,
            kernel_stack_size,
            user_entry: 0,
            user_stack: 0,
            fork_user_rflags: 0,
            fs_base: 0,
            in_syscall: false,
            syscall_user_cr3: 0,
        }
    }

    /// 新しいユーザーモードスレッドを作成
    ///
    /// # Arguments
    /// * `process_id` - 所属するプロセスID
    /// * `name` - スレッド名
    /// * `user_entry` - ユーザーモードのエントリーポイント
    /// * `user_stack` - ユーザースタックのトップアドレス
    /// * `kernel_stack` - カーネルスタックのアドレス
    /// * `kernel_stack_size` - カーネルスタックのサイズ
    pub fn new_usermode(
        process_id: ProcessId,
        name: &str,
        user_entry: u64,
        user_stack: u64,
        kernel_stack: u64,
        kernel_stack_size: usize,
    ) -> Self {
        let mut name_buf = [0u8; 32];
        let bytes = name.as_bytes();
        let len = core::cmp::min(bytes.len(), 32);
        name_buf[..len].copy_from_slice(&bytes[..len]);

        // カーネルスタックを設定（ユーザーモードからシステムコール時に使用）
        let mut context = Context::new();
        let stack_top = (kernel_stack + kernel_stack_size as u64) & !0xF;

        // ユーザーモードへジャンプするトランポリン関数を設定
        extern "C" fn usermode_entry_trampoline() -> ! {
            // この関数は各スレッドが最初に実行される
            // スレッド固有のuser_entryとuser_stackを取得してジャンプする
            let tid = match current_thread_id() {
                Some(t) => t,
                None => {
                    crate::warn!("usermode_entry_trampoline: No current thread");
                    loop {
                        x86_64::instructions::hlt();
                    }
                }
            };
            let (entry, stack) =
                match with_thread(tid, |thread| (thread.user_entry(), thread.user_stack())) {
                    Some(v) => v,
                    None => {
                        crate::warn!("usermode_entry_trampoline: Thread not found");
                        loop {
                            x86_64::instructions::hlt();
                        }
                    }
                };

            crate::debug!(
                "Jumping to usermode: entry={:#x}, stack={:#x}",
                entry,
                stack
            );
            unsafe {
                crate::task::jump_to_usermode(entry, stack);
            }
        }

        let stack_ptr = stack_top - 8;
        unsafe {
            let ret_addr = stack_ptr as *mut u64;
            *ret_addr = thread_exit_handler as *const () as u64;
        }

        context.rsp = stack_ptr;
        context.rbp = stack_top;
        context.rip = usermode_entry_trampoline as *const () as u64;
        context.rflags = 0x202;

        crate::debug!(
            "Creating usermode thread '{}': user_entry={:#x}, user_stack={:#x}",
            name,
            user_entry,
            user_stack
        );

        Self {
            id: ThreadId::new(),
            process_id,
            name: name_buf,
            name_len: len,
            state: ThreadState::Ready,
            context,
            kernel_stack,
            kernel_stack_size,
            user_entry,
            user_stack,
            fork_user_rflags: 0,
            fs_base: 0,
            in_syscall: false,
            syscall_user_cr3: 0,
        }
    }

    /// ユーザーモードエントリポイントを取得
    pub fn user_entry(&self) -> u64 {
        self.user_entry
    }

    /// ユーザースタックを取得
    pub fn user_stack(&self) -> u64 {
        self.user_stack
    }

    /// TLS FSベースを取得
    pub fn fs_base(&self) -> u64 {
        self.fs_base
    }

    /// fork_user_rflags を取得
    pub fn fork_user_rflags(&self) -> u64 {
        self.fork_user_rflags
    }

    /// fork の子プロセス用スレッドを作成
    ///
    /// 子スレッドはユーザー空間で fork() の戻り値として 0 を返す
    pub fn new_fork_child(
        process_id: ProcessId,
        user_rip: u64,
        user_rsp: u64,
        user_rflags: u64,
        fs_base: u64,
        kernel_stack: u64,
        kernel_stack_size: usize,
    ) -> Self {
        let mut context = Context::new();
        let stack_top = (kernel_stack + kernel_stack_size as u64) & !0xF;
        let stack_ptr = stack_top - 8;
        unsafe {
            let ret_addr = stack_ptr as *mut u64;
            *ret_addr = thread_exit_handler as *const () as u64;
        }
        context.rsp = stack_ptr;
        context.rbp = stack_top;

        extern "C" fn fork_child_trampoline() -> ! {
            let tid = match current_thread_id() {
                Some(t) => t,
                None => {
                    crate::warn!("fork_child_trampoline: No current thread");
                    loop {
                        x86_64::instructions::hlt();
                    }
                }
            };
            let (entry, stack, rflags, fs) = match with_thread(tid, |thread| {
                (
                    thread.user_entry(),
                    thread.user_stack(),
                    thread.fork_user_rflags(),
                    thread.fs_base(),
                )
            }) {
                Some(v) => v,
                None => {
                    crate::warn!("fork_child_trampoline: Thread not found");
                    loop {
                        x86_64::instructions::hlt();
                    }
                }
            };
            unsafe {
                crate::task::usermode::jump_to_usermode_fork_child(entry, stack, rflags, fs);
            }
        }

        context.rip = fork_child_trampoline as *const () as u64;
        context.rflags = 0x202;

        Self {
            id: ThreadId::new(),
            process_id,
            name: [
                b'f', b'o', b'r', b'k', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0,
            ],
            name_len: 4,
            state: ThreadState::Ready,
            context,
            kernel_stack,
            kernel_stack_size,
            user_entry: user_rip,
            user_stack: user_rsp,
            fork_user_rflags: user_rflags,
            fs_base,
            in_syscall: false,
            syscall_user_cr3: 0,
        }
    }

    /// TLS FSベースを設定
    pub fn set_fs_base(&mut self, base: u64) {
        self.fs_base = base;
    }

    /// システムコールコンテキスト中かどうか
    pub fn in_syscall(&self) -> bool {
        self.in_syscall
    }

    pub fn set_in_syscall(&mut self, in_syscall: bool) {
        self.in_syscall = in_syscall;
    }

    pub fn syscall_user_cr3(&self) -> u64 {
        self.syscall_user_cr3
    }

    pub fn set_syscall_user_cr3(&mut self, cr3: u64) {
        self.syscall_user_cr3 = cr3;
    }

    /// スレッドIDを取得
    pub fn id(&self) -> ThreadId {
        self.id
    }

    /// 所属するプロセスIDを取得
    pub fn process_id(&self) -> ProcessId {
        self.process_id
    }

    /// スレッド名を取得
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("???")
    }

    /// スレッドの状態を取得
    pub fn state(&self) -> ThreadState {
        self.state
    }

    /// スレッドの状態を設定
    pub fn set_state(&mut self, state: ThreadState) {
        self.state = state;
    }

    /// コンテキストへの可変参照を取得
    pub fn context_mut(&mut self) -> &mut Context {
        &mut self.context
    }

    /// コンテキストへの参照を取得
    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn kernel_stack_top(&self) -> u64 {
        (self.kernel_stack + self.kernel_stack_size as u64) & !0xF
    }
}

impl core::fmt::Debug for Thread {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Thread")
            .field("id", &self.id)
            .field("process_id", &self.process_id)
            .field("name", &self.name)
            .field("state", &self.state)
            .field("kernel_stack", &format_args!("{:#x}", self.kernel_stack))
            .field("kernel_stack_size", &self.kernel_stack_size)
            .finish()
    }
}

/// スレッドキュー
///
/// 実行可能なスレッドを管理するキュー
pub struct ThreadQueue {
    /// スレッドの配列（最大容量）
    threads: [Option<Thread>; Self::MAX_THREADS],
    /// 現在のスレッド数
    count: usize,
}

impl ThreadQueue {
    /// スレッドキューの最大容量
    pub const MAX_THREADS: usize = 64;

    /// 新しいスレッドキューを作成
    pub const fn new() -> Self {
        const INIT: Option<Thread> = None;
        Self {
            threads: [INIT; Self::MAX_THREADS],
            count: 0,
        }
    }

    /// スレッドを追加
    ///
    /// # Returns
    /// 成功時はスレッドIDを返す。キューが満杯の場合はNone
    pub fn push(&mut self, thread: Thread) -> Option<ThreadId> {
        if self.count >= Self::MAX_THREADS {
            return None;
        }

        let id = thread.id();

        // 空きスロットを探す
        for slot in &mut self.threads {
            if slot.is_none() {
                *slot = Some(thread);
                self.count += 1;
                return Some(id);
            }
        }

        None
    }

    /// スレッドIDでスレッドを取得
    pub fn get(&self, id: ThreadId) -> Option<&Thread> {
        self.threads
            .iter()
            .find_map(|slot| slot.as_ref().filter(|t| t.id() == id))
    }

    /// スレッドIDでスレッドの可変参照を取得
    pub fn get_mut(&mut self, id: ThreadId) -> Option<&mut Thread> {
        self.threads
            .iter_mut()
            .find_map(|slot| slot.as_mut().filter(|t| t.id() == id))
    }

    /// スレッドを削除
    ///
    /// # Returns
    /// 削除されたスレッドを返す。存在しない場合はNone
    pub fn remove(&mut self, id: ThreadId) -> Option<Thread> {
        for slot in &mut self.threads {
            if let Some(ref thread) = slot {
                if thread.id() == id {
                    self.count -= 1;
                    return slot.take();
                }
            }
        }
        None
    }

    /// 次に実行すべきスレッドを取得（削除せずに参照を返す）
    ///
    /// Ready状態のスレッドを優先して返す
    pub fn peek_next(&self) -> Option<&Thread> {
        // Ready状態のスレッドを探す
        self.threads
            .iter()
            .filter_map(|slot| slot.as_ref())
            .find(|t| t.state() == ThreadState::Ready)
    }

    /// 次に実行すべきスレッドを取得（可変参照）
    pub fn peek_next_mut(&mut self) -> Option<&mut Thread> {
        // Ready状態のスレッドを探す
        self.threads
            .iter_mut()
            .filter_map(|slot| slot.as_mut())
            .find(|t| t.state() == ThreadState::Ready)
    }

    /// 指定されたスレッドの次のReady状態のスレッドを取得（ラウンドロビン用）
    ///
    /// current_idの次のスロットから検索を開始し、見つからなければ先頭から検索
    pub fn peek_next_after(&mut self, current_id: Option<ThreadId>) -> Option<&mut Thread> {
        if let Some(current) = current_id {
            // 現在のスレッドのインデックスを探す
            let mut current_index = None;
            for (i, slot) in self.threads.iter().enumerate() {
                if let Some(thread) = slot.as_ref() {
                    if thread.id() == current {
                        current_index = Some(i);
                        break;
                    }
                }
            }

            if let Some(start_idx) = current_index {
                for i in (start_idx + 1..Self::MAX_THREADS).chain(0..=start_idx) {
                    if self.threads[i]
                        .as_ref()
                        .is_some_and(|t| t.state() == ThreadState::Ready)
                    {
                        return self.threads[i].as_mut();
                    }
                }
            }
        }

        // current_idがない場合は最初のReady状態のスレッドを返す
        self.peek_next_mut()
    }

    /// 指定された状態のスレッド数をカウント
    pub fn count_by_state(&self, state: ThreadState) -> usize {
        self.threads
            .iter()
            .filter_map(|slot| slot.as_ref())
            .filter(|t| t.state() == state)
            .count()
    }

    /// 指定されたプロセスに属するスレッドを反復処理
    pub fn iter_by_process(&self, process_id: ProcessId) -> impl Iterator<Item = &Thread> {
        self.threads
            .iter()
            .filter_map(|slot| slot.as_ref())
            .filter(move |t| t.process_id() == process_id)
    }

    /// すべてのスレッドを反復処理
    pub fn iter(&self) -> impl Iterator<Item = &Thread> {
        self.threads.iter().filter_map(|slot| slot.as_ref())
    }

    /// すべてのスレッドを可変反復処理
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Thread> {
        self.threads.iter_mut().filter_map(|slot| slot.as_mut())
    }

    /// 現在のスレッド数を取得
    pub fn count(&self) -> usize {
        self.count
    }

    /// スレッドキューが満杯かどうか
    pub fn is_full(&self) -> bool {
        self.count >= Self::MAX_THREADS
    }

    /// スレッドキューが空かどうか
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// グローバルスレッドキュー
pub(super) static THREAD_QUEUE: SpinLock<ThreadQueue> = SpinLock::new(ThreadQueue::new());

/// 現在実行中のスレッドID
pub(super) static CURRENT_THREAD: SpinLock<Option<ThreadId>> = SpinLock::new(None);

/// スレッドキューにスレッドを追加
pub fn add_thread(thread: Thread) -> Option<ThreadId> {
    THREAD_QUEUE.lock().push(thread)
}

/// スレッドIDでスレッド情報を取得（読み取り専用操作）
pub fn with_thread<F, R>(id: ThreadId, f: F) -> Option<R>
where
    F: FnOnce(&Thread) -> R,
{
    let queue = THREAD_QUEUE.lock();
    queue.get(id).map(f)
}

/// スレッドIDでスレッド情報を可変操作
pub fn with_thread_mut<F, R>(id: ThreadId, f: F) -> Option<R>
where
    F: FnOnce(&mut Thread) -> R,
{
    let mut queue = THREAD_QUEUE.lock();
    queue.get_mut(id).map(f)
}

/// スレッドを削除
pub fn remove_thread(id: ThreadId) -> Option<Thread> {
    THREAD_QUEUE.lock().remove(id)
}

/// 次に実行すべきスレッドIDを取得
pub fn peek_next_thread() -> Option<ThreadId> {
    THREAD_QUEUE.lock().peek_next().map(|t| t.id())
}

/// 指定された状態のスレッド数を取得
pub fn count_threads_by_state(state: ThreadState) -> usize {
    THREAD_QUEUE.lock().count_by_state(state)
}

/// すべてのスレッドに対して操作を実行
pub fn for_each_thread<F>(mut f: F)
where
    F: FnMut(&Thread),
{
    let queue = THREAD_QUEUE.lock();
    for thread in queue.iter() {
        f(thread);
    }
}

/// 現在のスレッド数を取得
pub fn thread_count() -> usize {
    THREAD_QUEUE.lock().count()
}

/// 指定した u64 IDのスレッドが存在するか確認 (IPC送信先検証用)
pub fn thread_id_exists(id_val: u64) -> bool {
    let queue = THREAD_QUEUE.lock();
    let exists = queue.iter().any(|t| t.id().as_u64() == id_val);
    exists
}

/// 現在実行中のスレッドIDを取得
pub fn current_thread_id() -> Option<ThreadId> {
    *CURRENT_THREAD.lock()
}

/// 現在実行中のスレッドIDを設定
pub fn set_current_thread(id: Option<ThreadId>) {
    *CURRENT_THREAD.lock() = id;
}
