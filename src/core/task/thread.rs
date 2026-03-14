use crate::interrupt::spinlock::SpinLock;
use x86_64::VirtAddr;

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
    /// 直近の SYSCALL 入口で保存したユーザー RIP
    syscall_user_rip: u64,
    /// 直近の SYSCALL 入口で保存したユーザー RSP
    syscall_user_rsp: u64,
    /// 直近の SYSCALL 入口で保存したユーザー RFLAGS
    syscall_user_rflags: u64,
    /// futex wait timeout で起床したことを示すフラグ
    futex_timed_out: bool,
    /// IPC受信などで眠る前に起床要求が来たことを示すフラグ
    pending_wakeup: bool,
}

// Simple kernel stack pool for creating kernel stacks for threads
const KSTACK_POOL_SIZE: usize = 4096 * 64; // 256 KiB（最大約12スレッド分、フリーリストで再利用）
const KSTACK_PAGE_BYTES: usize = 4096;
const KSTACK_GUARD_BYTES: usize = KSTACK_PAGE_BYTES;

/// 解放済みカーネルスタックのフリーリスト
/// 各エントリは guard_addr（= スタックベース - KSTACK_GUARD_BYTES）を格納。0 = 空き
const KSTACK_FREE_LIST_CAP: usize = 32;
static KSTACK_FREE_LIST: SpinLock<[u64; KSTACK_FREE_LIST_CAP]> =
    SpinLock::new([0u64; KSTACK_FREE_LIST_CAP]);

#[repr(align(4096))]
struct KernelStackPool([u8; KSTACK_POOL_SIZE]);

static KSTACK_POOL: SpinLock<KernelStackPool> =
    SpinLock::new(KernelStackPool([0; KSTACK_POOL_SIZE]));
static NEXT_KSTACK_OFFSET: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

fn unmap_guard_page(guard_addr: u64) -> bool {
    use x86_64::structures::paging::mapper::Translate;
    use x86_64::structures::paging::mapper::TranslateError;
    use x86_64::structures::paging::{Mapper, Page, Size4KiB};

    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(guard_addr));
    let mut page_table_lock = crate::mem::paging::PAGE_TABLE.lock();
    let page_table = match page_table_lock.as_mut() {
        Some(pt) => pt,
        None => return false,
    };

    match page_table.translate_page(page) {
        Ok(_) => {}
        Err(TranslateError::PageNotMapped) => return true,
        Err(_) => return false,
    }

    unsafe {
        page_table
            .unmap(page)
            .map(|(_frame, flush)| {
                flush.flush();
                true
            })
            .unwrap_or(false)
    }
}

/// カーネルスタックを内部プールから割り当てます。
/// フリーリストに空きがあれば再利用し、なければバンプアロケータから新規割り当て。
/// Returns base address (bottom) of stack.
pub fn allocate_kernel_stack(size: usize) -> Option<u64> {
    if size == 0 || size > KSTACK_POOL_SIZE.saturating_sub(KSTACK_GUARD_BYTES) {
        return None;
    }
    let size_pages = size
        .checked_add(KSTACK_PAGE_BYTES - 1)?
        .checked_div(KSTACK_PAGE_BYTES)?
        .checked_mul(KSTACK_PAGE_BYTES)?;

    // フリーリストから再利用を試みる（guard ページは既に unmap 済み）
    {
        let mut list = KSTACK_FREE_LIST.lock();
        for slot in list.iter_mut() {
            if *slot != 0 {
                let guard_addr = *slot;
                *slot = 0;
                return guard_addr.checked_add(KSTACK_GUARD_BYTES as u64);
            }
        }
    }

    // バンプアロケータから新規割り当て
    let alloc_size = size_pages.checked_add(KSTACK_GUARD_BYTES)?;
    let off = NEXT_KSTACK_OFFSET.fetch_add(alloc_size, core::sync::atomic::Ordering::SeqCst);
    if off + alloc_size > KSTACK_POOL_SIZE {
        return None;
    }

    let pool_base = {
        let pool = KSTACK_POOL.lock();
        pool.0.as_ptr() as u64
    };
    let guard_addr = pool_base.checked_add(off as u64)?;
    if !unmap_guard_page(guard_addr) {
        return None;
    }

    guard_addr.checked_add(KSTACK_GUARD_BYTES as u64)
}

/// カーネルスタックをフリーリストへ返却する。
/// `base` は `allocate_kernel_stack` が返したアドレス（ガードページの直上）。
pub fn free_kernel_stack(base: u64) {
    if base == 0 {
        return;
    }
    let guard_addr = match base.checked_sub(KSTACK_GUARD_BYTES as u64) {
        Some(a) => a,
        None => return,
    };
    // プール範囲内のアドレスのみ受け付ける
    let pool_base = {
        let pool = KSTACK_POOL.lock();
        pool.0.as_ptr() as u64
    };
    if guard_addr < pool_base || guard_addr >= pool_base + KSTACK_POOL_SIZE as u64 {
        return;
    }
    let mut list = KSTACK_FREE_LIST.lock();
    for slot in list.iter_mut() {
        if *slot == 0 {
            *slot = guard_addr;
            return;
        }
    }
    // フリーリストが満杯の場合はリークさせる（通常は発生しない）
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
        context.rip = entry_point as usize as u64;

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
            syscall_user_rip: 0,
            syscall_user_rsp: 0,
            syscall_user_rflags: 0,
            futex_timed_out: false,
            pending_wakeup: false,
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
            syscall_user_rip: 0,
            syscall_user_rsp: 0,
            syscall_user_rflags: 0,
            futex_timed_out: false,
            pending_wakeup: false,
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
            syscall_user_rip: user_rip,
            syscall_user_rsp: user_rsp,
            syscall_user_rflags: user_rflags,
            futex_timed_out: false,
            pending_wakeup: false,
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

    pub fn syscall_user_context(&self) -> (u64, u64, u64) {
        (
            self.syscall_user_rip,
            self.syscall_user_rsp,
            self.syscall_user_rflags,
        )
    }

    pub fn set_syscall_user_context(&mut self, rip: u64, rsp: u64, rflags: u64) {
        self.syscall_user_rip = rip;
        self.syscall_user_rsp = rsp;
        self.syscall_user_rflags = rflags;
    }

    pub fn set_futex_timed_out(&mut self, timed_out: bool) {
        self.futex_timed_out = timed_out;
    }

    pub fn take_futex_timed_out(&mut self) -> bool {
        let timed_out = self.futex_timed_out;
        self.futex_timed_out = false;
        timed_out
    }

    /// 起床要求フラグを立てる（眠る前に wake が呼ばれた場合の競合回避）
    pub fn set_pending_wakeup(&mut self) {
        self.pending_wakeup = true;
    }

    /// 起床要求フラグを取り出して消去する。true なら眠る必要はない。
    pub fn take_pending_wakeup(&mut self) -> bool {
        let v = self.pending_wakeup;
        self.pending_wakeup = false;
        v
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

    pub fn kernel_stack_bottom(&self) -> u64 {
        self.kernel_stack
    }

    pub fn is_kernel_stack_guard_intact(&self) -> bool {
        let (pool_start, pool_end) = {
            let pool = KSTACK_POOL.lock();
            let start = pool.0.as_ptr() as u64;
            (start, start + KSTACK_POOL_SIZE as u64)
        };
        let stack_end = match self.kernel_stack.checked_add(self.kernel_stack_size as u64) {
            Some(v) => v,
            None => return false,
        };
        let pooled_stack = self.kernel_stack >= pool_start + KSTACK_GUARD_BYTES as u64
            && stack_end <= pool_end
            && self.kernel_stack >= KSTACK_GUARD_BYTES as u64;
        if !pooled_stack {
            return true;
        }
        let guard_start = self.kernel_stack - KSTACK_GUARD_BYTES as u64;
        crate::mem::paging::translate_addr(VirtAddr::new(guard_start)).is_none()
    }

    /// カーネルスタックのベースアドレス（フリーリスト返却用）
    pub fn kernel_stack_base(&self) -> u64 {
        self.kernel_stack
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
    /// スロット世代番号（スロット再利用時に増加）
    slot_generations: [u64; Self::MAX_THREADS],
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
            slot_generations: [0; Self::MAX_THREADS],
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
        for (idx, slot) in self.threads.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(thread);
                self.slot_generations[idx] = self.slot_generations[idx].wrapping_add(1);
                if self.slot_generations[idx] == 0 {
                    self.slot_generations[idx] = 1;
                }
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

    /// スレッドIDが存在するスロットインデックスを返す
    pub fn slot_index(&self, id: ThreadId) -> Option<usize> {
        self.threads
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|t| t.id() == id))
    }

    /// スレッドIDが存在するスロットと世代番号を返す
    pub fn slot_index_and_generation(&self, id: ThreadId) -> Option<(usize, u64)> {
        self.threads.iter().enumerate().find_map(|(idx, slot)| {
            slot.as_ref()
                .filter(|t| t.id() == id)
                .map(|_| (idx, self.slot_generations[idx]))
        })
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

impl Default for ThreadQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// グローバルスレッドキュー
pub(super) static THREAD_QUEUE: SpinLock<ThreadQueue> = SpinLock::new(ThreadQueue::new());

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

/// 指定したスレッドIDのスロットインデックスを返す
pub fn thread_slot_index(id: ThreadId) -> Option<usize> {
    THREAD_QUEUE.lock().slot_index(id)
}

/// 指定したu64スレッドIDのスロットインデックスを返す
pub fn thread_slot_index_by_u64(id_val: u64) -> Option<usize> {
    thread_slot_index(ThreadId::from_u64(id_val))
}

/// 指定したスレッドIDのスロットインデックスと世代番号を返す
pub fn thread_slot_index_and_generation(id: ThreadId) -> Option<(usize, u64)> {
    THREAD_QUEUE.lock().slot_index_and_generation(id)
}

/// 指定したu64スレッドIDのスロットインデックスと世代番号を返す
pub fn thread_slot_index_and_generation_by_u64(id_val: u64) -> Option<(usize, u64)> {
    thread_slot_index_and_generation(ThreadId::from_u64(id_val))
}

/// 現在実行中のスレッドIDを取得
pub fn current_thread_id() -> Option<ThreadId> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let raw = crate::percpu::current_thread_raw_id();
        if raw == 0 {
            None
        } else {
            Some(ThreadId::from_u64(raw))
        }
    })
}

/// 現在実行中のスレッドIDを設定
pub fn set_current_thread(id: Option<ThreadId>) {
    let raw = id.map(|v| v.as_u64()).unwrap_or(0);
    x86_64::instructions::interrupts::without_interrupts(|| {
        crate::percpu::set_current_thread_raw_id(raw);
    });
}
