use crate::interrupt::spinlock::SpinLock;

use super::ids::{PrivilegeLevel, ProcessId, ProcessState};
use super::signal::SignalState;
use super::fd_table::FdTable;

/// プロセス構造体
///
/// メモリ空間とリソースを管理する実行単位。
/// 1つ以上のスレッドを持つ。
pub struct Process {
    /// プロセスID
    id: ProcessId,
    /// プロセス名 (固定長バッファ)
    name: [u8; 32],
    /// 有効な名前の長さ
    name_len: usize,
    /// プロセスの状態
    state: ProcessState,
    /// 権限レベル
    privilege: PrivilegeLevel,
    /// 親プロセスID（存在する場合）
    parent_id: Option<ProcessId>,
    /// ページテーブルのアドレス（メモリ空間）。Noneの場合はカーネル空間を共有。
    page_table: Option<u64>,
    /// ヒープ開始アドレス
    heap_start: u64,
    /// 現在のヒープ終了アドレス (program break)
    heap_end: u64,
    /// ユーザースタックの現在の最低マップアドレス（下向きに伸びる）
    stack_bottom: u64,
    /// ユーザースタックのトップアドレス（初期 RSP 付近）
    stack_top: u64,
    /// カレントワーキングディレクトリ（固定バッファ、ヒープ確保不要）
    cwd: [u8; 256],
    cwd_len: usize,
    /// 優先度（0が最高、値が大きいほど低い）
    priority: u8,
    /// 終了コード（生存中はNone）
    exit_code: Option<u64>,
    /// シグナル状態（ハンドラ・マスク・pending）— ヒープに置いてスタック消費を抑える
    signal_state: alloc::boxed::Box<SignalState>,
    /// プロセスごとのファイルディスクリプタテーブル — ヒープに置いてスタック消費を抑える
    fd_table: alloc::boxed::Box<FdTable>,
}

impl Process {
    /// 新しいプロセスを作成
    ///
    /// # Arguments
    /// * `name` - プロセス名
    /// * `privilege` - 権限レベル
    /// * `parent_id` - 親プロセスID
    /// * `priority` - プロセスの優先度
    pub fn new(
        name: &str,
        privilege: PrivilegeLevel,
        parent_id: Option<ProcessId>,
        priority: u8,
    ) -> Self {
        let mut name_buf = [0u8; 32];
        let bytes = name.as_bytes();
        let len = core::cmp::min(bytes.len(), 32);
        name_buf[..len].copy_from_slice(&bytes[..len]);

        // デフォルトのヒープ領域（仮）。exec時に再設定されるべき。
        // 0x40000000番地あたりを開始にする例が多いが、ここでは0にしておく。
        let heap_start = 0;

        Self {
            id: ProcessId::new(),
            name: name_buf,
            name_len: len,
            state: ProcessState::Running,
            privilege,
            parent_id,
            page_table: None, // TODO: ページテーブル実装後に設定
            heap_start,
            heap_end: heap_start,
            stack_bottom: 0,
            stack_top: 0,
            cwd: {
                let mut b = [0u8; 256];
                b[0] = b'/';
                b
            },
            cwd_len: 1,
            priority,
            exit_code: None,
            signal_state: alloc::boxed::Box::new(SignalState::new()),
            fd_table: FdTable::new_boxed(),
        }
    }

    /// プロセスIDを取得
    pub fn id(&self) -> ProcessId {
        self.id
    }

    /// プロセス名を取得
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("???")
    }

    /// プロセスの状態を取得
    pub fn state(&self) -> ProcessState {
        self.state
    }

    /// プロセスの状態を設定
    pub fn set_state(&mut self, state: ProcessState) {
        self.state = state;
    }

    /// 権限レベルを取得
    pub fn privilege(&self) -> PrivilegeLevel {
        self.privilege
    }

    /// 親プロセスIDを取得
    pub fn parent_id(&self) -> Option<ProcessId> {
        self.parent_id
    }

    /// 優先度を取得
    pub fn priority(&self) -> u8 {
        self.priority
    }

    /// 終了コードを取得
    pub fn exit_code(&self) -> Option<u64> {
        self.exit_code
    }

    /// 終了状態へ遷移
    pub fn mark_exited(&mut self, exit_code: u64) {
        self.state = ProcessState::Zombie;
        self.exit_code = Some(exit_code);
    }

    /// ページテーブルアドレスを取得
    pub fn page_table(&self) -> Option<u64> {
        self.page_table
    }

    /// ページテーブルアドレスを設定
    pub fn set_page_table(&mut self, page_table: u64) {
        self.page_table = Some(page_table);
    }

    /// ヒープ終了アドレスを取得
    pub fn heap_end(&self) -> u64 {
        self.heap_end
    }

    /// ヒープ終了アドレスを設定
    pub fn set_heap_end(&mut self, addr: u64) {
        self.heap_end = addr;
    }

    /// ヒープ開始アドレスを取得
    pub fn heap_start(&self) -> u64 {
        self.heap_start
    }

    /// ヒープ開始アドレスを設定
    pub fn set_heap_start(&mut self, addr: u64) {
        self.heap_start = addr;
    }

    pub fn stack_bottom(&self) -> u64 { self.stack_bottom }
    pub fn stack_top(&self) -> u64 { self.stack_top }
    pub fn set_stack_bottom(&mut self, addr: u64) { self.stack_bottom = addr; }
    pub fn set_stack_top(&mut self, addr: u64) { self.stack_top = addr; }

    pub fn cwd(&self) -> &str {
        core::str::from_utf8(&self.cwd[..self.cwd_len]).unwrap_or("/")
    }

    pub fn set_cwd(&mut self, path: &str) {
        let bytes = path.as_bytes();
        let len = bytes.len().min(255);
        self.cwd[..len].copy_from_slice(&bytes[..len]);
        self.cwd_len = len;
    }

    /// シグナル状態への読み取りアクセス
    pub fn signal_state(&self) -> &SignalState {
        &self.signal_state
    }

    /// シグナル状態への可変アクセス
    pub fn signal_state_mut(&mut self) -> &mut SignalState {
        &mut self.signal_state
    }

    /// FD テーブルへの読み取りアクセス
    pub fn fd_table(&self) -> &FdTable {
        &self.fd_table
    }

    /// FD テーブルへの可変アクセス
    pub fn fd_table_mut(&mut self) -> &mut FdTable {
        &mut self.fd_table
    }

    /// fork 用: FD テーブルをクローンして新しい Box を返す
    pub fn clone_fd_table_for_fork(&self) -> alloc::boxed::Box<FdTable> {
        self.fd_table.clone_for_fork()
    }

    /// FD テーブルを差し替える（fork の子プロセス初期化で使用）
    pub fn set_fd_table(&mut self, table: alloc::boxed::Box<FdTable>) {
        self.fd_table = table;
    }
}

impl core::fmt::Debug for Process {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut debug_struct = f.debug_struct("Process");
        debug_struct
            .field("id", &self.id)
            .field("name", &self.name())
            .field("state", &self.state)
            .field("privilege", &self.privilege)
            .field("parent_id", &self.parent_id)
            .field("priority", &self.priority)
            .field("exit_code", &self.exit_code);

        if let Some(pt) = self.page_table {
            debug_struct.field("page_table", &format_args!("{:#x}", pt));
        } else {
            debug_struct.field("page_table", &None::<u64>);
        }

        debug_struct.finish()
    }
}

/// プロセステーブル
///
/// システム内のすべてのプロセスを管理する
pub struct ProcessTable {
    /// プロセスの配列（最大容量）
    processes: [Option<Process>; Self::MAX_PROCESSES],
    /// 現在のプロセス数
    count: usize,
}

impl ProcessTable {
    /// プロセステーブルの最大容量
    pub const MAX_PROCESSES: usize = 64;

    /// 新しいプロセステーブルを作成
    pub const fn new() -> Self {
        const INIT: Option<Process> = None;
        Self {
            processes: [INIT; Self::MAX_PROCESSES],
            count: 0,
        }
    }

    /// プロセスを追加
    ///
    /// # Returns
    /// 成功時はプロセスIDを返す。テーブルが満杯の場合はNone
    pub fn add(&mut self, process: Process) -> Option<ProcessId> {
        if self.count >= Self::MAX_PROCESSES {
            return None;
        }

        let id = process.id();

        // 空きスロットを探す
        for slot in &mut self.processes {
            if slot.is_none() {
                *slot = Some(process);
                self.count += 1;
                return Some(id);
            }
        }

        None
    }

    /// プロセスIDでプロセスを取得
    pub fn get(&self, id: ProcessId) -> Option<&Process> {
        self.processes
            .iter()
            .find_map(|slot| slot.as_ref().filter(|p| p.id() == id))
    }

    /// プロセスIDでプロセスの可変参照を取得
    pub fn get_mut(&mut self, id: ProcessId) -> Option<&mut Process> {
        self.processes
            .iter_mut()
            .find_map(|slot| slot.as_mut().filter(|p| p.id() == id))
    }

    /// プロセスを削除
    ///
    /// # Returns
    /// 削除されたプロセスを返す。存在しない場合はNone
    pub fn remove(&mut self, id: ProcessId) -> Option<Process> {
        for slot in &mut self.processes {
            if let Some(ref process) = slot {
                if process.id() == id {
                    self.count -= 1;
                    return slot.take();
                }
            }
        }
        None
    }

    /// すべてのプロセスを反復処理
    pub fn iter(&self) -> impl Iterator<Item = &Process> {
        self.processes.iter().filter_map(|slot| slot.as_ref())
    }

    /// すべてのプロセスを可変反復処理
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Process> {
        self.processes.iter_mut().filter_map(|slot| slot.as_mut())
    }

    /// 名前でプロセスを検索
    pub fn find_by_name(&self, name: &str) -> Option<&Process> {
        // 名前比較（簡易実装: 完全一致のみ考慮）
        // 注: Processの名前に .service などの拡張子を含む場合があるため
        // ここでは前方一致などで緩和するのも手だが、厳密には完全一致で。
        self.processes
            .iter()
            .filter_map(|slot| slot.as_ref())
            .find(|p| p.name() == name)
    }

    fn is_child_match(process: &Process, parent: ProcessId, target: Option<ProcessId>) -> bool {
        if process.parent_id() != Some(parent) {
            return false;
        }
        if let Some(target_id) = target {
            process.id() == target_id
        } else {
            true
        }
    }

    /// 対象に一致する子プロセスが存在するかを返す
    pub fn has_child(&self, parent: ProcessId, target: Option<ProcessId>) -> bool {
        self.processes
            .iter()
            .filter_map(|slot| slot.as_ref())
            .any(|p| Self::is_child_match(p, parent, target))
    }

    /// ゾンビ子プロセスを1つ回収する
    pub fn reap_zombie_child(
        &mut self,
        parent: ProcessId,
        target: Option<ProcessId>,
    ) -> Option<(ProcessId, u64, Option<u64>)> {
        for slot in &mut self.processes {
            let should_reap = slot.as_ref().is_some_and(|proc| {
                Self::is_child_match(proc, parent, target) && proc.state() == ProcessState::Zombie
            });
            if !should_reap {
                continue;
            }

            if let Some(proc) = slot.take() {
                let pid = proc.id();
                let exit_code = proc.exit_code().unwrap_or(0);
                let page_table = proc.page_table();
                self.count = self.count.saturating_sub(1);
                return Some((pid, exit_code, page_table));
            }
        }
        None
    }

    /// 現在のプロセス数を取得
    pub fn count(&self) -> usize {
        self.count
    }
}

impl Default for ProcessTable {
    fn default() -> Self {
        Self::new()
    }
}

/// グローバルプロセステーブル
static PROCESS_TABLE: SpinLock<ProcessTable> = SpinLock::new(ProcessTable::new());

/// プロセステーブルにプロセスを追加
pub fn add_process(process: Process) -> Option<ProcessId> {
    PROCESS_TABLE.lock().add(process)
}

/// プロセスを削除
pub fn remove_process(id: ProcessId) -> Option<Process> {
    PROCESS_TABLE.lock().remove(id)
}

/// プロセスIDでプロセス情報を取得（読み取り専用操作）
pub fn with_process<F, R>(id: ProcessId, f: F) -> Option<R>
where
    F: FnOnce(&Process) -> R,
{
    let table = PROCESS_TABLE.lock();
    table.get(id).map(f)
}

/// プロセスIDでプロセス情報を可変操作
pub fn with_process_mut<F, R>(id: ProcessId, f: F) -> Option<R>
where
    F: FnOnce(&mut Process) -> R,
{
    let mut table = PROCESS_TABLE.lock();
    table.get_mut(id).map(f)
}

/// 名前からプロセスIDを検索
pub fn find_process_id_by_name(name: &str) -> Option<ProcessId> {
    let table = PROCESS_TABLE.lock();
    table.find_by_name(name).map(|p| p.id())
}

/// すべてのプロセスに対して処理を実行
pub fn for_each_process<F>(mut f: F)
where
    F: FnMut(&Process),
{
    let table = PROCESS_TABLE.lock();
    for process in table.iter() {
        f(process);
    }
}

/// プロセスを終了状態（Zombie）へ遷移させる
pub fn mark_process_exited(id: ProcessId, exit_code: u64) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(proc) = table.get_mut(id) {
        proc.mark_exited(exit_code);
    }
}

/// 一致する子プロセスが存在するか確認する
pub fn has_child_process(parent: ProcessId, target: Option<ProcessId>) -> bool {
    PROCESS_TABLE.lock().has_child(parent, target)
}

/// 一致するゾンビ子プロセスを回収する
pub fn reap_zombie_child_process(
    parent: ProcessId,
    target: Option<ProcessId>,
) -> Option<(ProcessId, u64)> {
    let (pid, exit_code, page_table) = PROCESS_TABLE.lock().reap_zombie_child(parent, target)?;
    if let Some(table_phys) = page_table {
        if let Err(e) = crate::mem::paging::destroy_user_page_table(table_phys) {
            crate::warn!(
                "Failed to destroy child page table while reaping pid={:?}: {:?}",
                pid,
                e
            );
        }
    }
    Some((pid, exit_code))
}

/// 現在のプロセス数を取得
pub fn process_count() -> usize {
    PROCESS_TABLE.lock().count()
}
