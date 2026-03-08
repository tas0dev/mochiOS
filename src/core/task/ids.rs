use core::sync::atomic::{AtomicU64, Ordering};

/// プロセスID生成用カウンタ
static NEXT_PROCESS_ID: AtomicU64 = AtomicU64::new(1);

/// スレッドID生成用カウンタ
static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);

/// プロセスID
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessId(u64);

impl ProcessId {
    /// 新しいプロセスIDを生成
    pub fn new() -> Self {
        Self(NEXT_PROCESS_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// プロセスIDの値を取得
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    /// 数値からProcessIdを生成（外部入力をIDとして扱う用途）
    pub const fn from_u64(id: u64) -> Self {
        Self(id)
    }
}

impl Default for ProcessId {
    fn default() -> Self {
        Self::new()
    }
}

/// スレッドID
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(u64);

impl ThreadId {
    /// 新しいスレッドIDを生成
    pub fn new() -> Self {
        Self(NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// スレッドIDの値を取得
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    /// 数値からThreadIdを生成
    pub const fn from_u64(id: u64) -> Self {
        Self(id)
    }
}

impl Default for ThreadId {
    fn default() -> Self {
        Self::new()
    }
}

/// スレッドの状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// 実行可能（スケジューラ待ち）
    Ready,
    /// 実行中
    Running,
    /// ブロック中（I/O待ちなど）
    Blocked,
    /// スリープ中
    Sleeping,
    /// 終了済み
    Terminated,
}

/// プロセスの状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// 実行中（少なくとも1つのスレッドがRunning/Ready）
    Running,
    /// スリープ中（すべてのスレッドがSleeping）
    Sleeping,
    /// ゾンビ（終了したが親に回収されていない）
    Zombie,
    /// 終了済み
    Terminated,
}

/// タスクが保有する権限レベル。ServiceとUserは区別のためであり、両方ともRing3で動作する。
///
/// - Core: カーネルモード（Ring0）で動作するタスク。システムの中核機能を担当。
/// - Service: ユーザーモード（Ring3）で動作するが、システムサービスやドライバを担当。
/// - User: ユーザーモード（Ring3）で動作。一般的なアプリケーションを担当。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivilegeLevel {
    /// コアレベルタスク（Ring0）
    Core,
    /// サービスレベルタスク（Ring3）
    Service,
    /// ユーザーレベルタスク（Ring3）
    User,
}
