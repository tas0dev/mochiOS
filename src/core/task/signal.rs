//! プロセスごとのシグナル状態

/// シグナルハンドラのデフォルト動作（SIG_DFL）
pub const SIG_DFL: u64 = 0;
/// シグナルを無視する（SIG_IGN）
pub const SIG_IGN: u64 = 1;

// ----- シグナル番号定数 (Linux x86-64 互換) -----
pub const SIGHUP:  usize = 1;
pub const SIGINT:  usize = 2;
pub const SIGQUIT: usize = 3;
pub const SIGKILL: usize = 9;
pub const SIGTERM: usize = 15;
pub const SIGCHLD: usize = 17;
pub const SIGCONT: usize = 18;
pub const SIGSTOP: usize = 19;
pub const SIGTSTP: usize = 20;
pub const SIGTTIN: usize = 21;
pub const SIGTTOU: usize = 22;
pub const SIGWINCH: usize = 28;

// ----- SA_* フラグ -----
pub const SA_RESTORER: u64 = 0x04000000;

/// シグナルのデフォルト動作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultAction {
    /// プロセスを終了する
    Terminate,
    /// シグナルを無視する
    Ignore,
}

/// シグナル番号に対応するデフォルト動作を返す
pub fn default_action(sig: usize) -> DefaultAction {
    match sig {
        // 無視するシグナル
        SIGCHLD | SIGCONT | SIGWINCH => DefaultAction::Ignore,
        SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => DefaultAction::Ignore,
        // それ以外はすべてプロセス終了
        _ => DefaultAction::Terminate,
    }
}

/// 1つのシグナルに対するアクション（Linux の struct sigaction と互換）
#[derive(Clone, Copy)]
pub struct SigAction {
    /// ハンドラ: SIG_DFL=0, SIG_IGN=1, それ以外はユーザー空間の関数ポインタ
    pub handler: u64,
    /// SA_* フラグ
    pub flags: u64,
    /// SA_RESTORER が設定されている場合のリストア関数ポインタ
    pub restorer: u64,
    /// ハンドラ実行中にブロックするシグナルマスク（ビット i = シグナル i+1）
    pub mask: u64,
}

impl SigAction {
    pub const fn default_action() -> Self {
        Self { handler: SIG_DFL, flags: 0, restorer: 0, mask: 0 }
    }

    pub fn is_default(&self) -> bool { self.handler == SIG_DFL }
    pub fn is_ignored(&self) -> bool { self.handler == SIG_IGN }
    pub fn has_user_handler(&self) -> bool { self.handler > SIG_IGN }
}

/// プロセスのシグナル状態
pub struct SignalState {
    /// シグナルごとのアクション（インデックス 0 = SIGHUP, ... インデックス 63 = シグナル64）
    pub actions: [SigAction; 64],
    /// ブロック中のシグナルマスク（ビット i = シグナル i+1 がブロック中）
    pub mask: u64,
    /// 保留（pending）シグナルビットマップ
    pub pending: u64,
}

impl SignalState {
    pub const fn new() -> Self {
        Self {
            actions: [SigAction::default_action(); 64],
            mask: 0,
            pending: 0,
        }
    }

    /// シグナルを pending にセットする
    pub fn set_pending(&mut self, sig: usize) {
        if sig >= 1 && sig <= 64 {
            self.pending |= 1u64 << (sig - 1);
        }
    }

    /// ブロックされていない pending シグナルを1つ取り出す（ビットをクリアして番号を返す）
    pub fn take_next_deliverable(&mut self) -> Option<usize> {
        let deliverable = self.pending & !self.mask;
        if deliverable == 0 {
            return None;
        }
        let bit = deliverable.trailing_zeros() as usize;
        self.pending &= !(1u64 << bit);
        Some(bit + 1)
    }

    /// 指定シグナルのアクションを取得
    pub fn action(&self, sig: usize) -> SigAction {
        if sig >= 1 && sig <= 64 {
            self.actions[sig - 1]
        } else {
            SigAction::default_action()
        }
    }

    /// 指定シグナルのアクションをセット
    pub fn set_action(&mut self, sig: usize, action: SigAction) {
        if sig >= 1 && sig <= 64 {
            self.actions[sig - 1] = action;
        }
    }
}
