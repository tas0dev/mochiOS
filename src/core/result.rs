//! mochiOSのResultとエラー型を定義
//!
//! すべてのカーネルエラーをResult型で表現し、panicを禁止

use core::fmt;

/// トップレベル
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kernel {
    /// メモリエラー
    Memory(Memory),
    /// プロセスエラー
    Process(Process),
    /// デバイスエラー
    Device(Device),
    /// ELFエラー
    Elf(Elf),
    /// 無効なパラメータ
    InvalidParam,
    /// 未実装の機能
    NotImplemented,
}

/// メモリ関連
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Memory {
    /// 利用可能なメモリがない
    OutOfMemory,
    /// 無効なアドレスへのアクセス
    InvalidAddress,
    /// メモリ保護違反
    PermissionDenied,
    /// 既にマップされたアドレス
    AlreadyMapped,
    /// マップされていないアドレス
    NotMapped,
    /// アライメントエラー
    AlignmentError,
}

/// プロセス関連
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Process {
    /// 無効なプロセスID
    InvalidPid,
    /// プロセスが見つからない
    ProcessNotFound,
    /// ゾンビプロセス
    ZombieProcess,
    /// プロセス数の上限に達した
    MaxProcessesReached,
    /// 権限不足
    InsufficientPrivilege,
    /// プロセス間通信エラー
    IpcError,
    /// タイムアウト
    Timeout,
    /// 暴走プロセス検出
    RogueProcessDetected,
    /// サービス関連
    Service(Service),
    /// プロセス作成完了
    CreationOk,
}

/// サービス関連
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// サービスが見つからない
    NotFound,
    /// サービスの起動失敗
    StartFailure,
    /// サービスの停止失敗
    StopFailure,
    /// サービスの応答なし
    NoResponse,
    /// サービスの権限不足
    InsufficientPrivilege,
    /// サービスの不正な状態
    InvalidState,
    /// サービスの競合
    Conflict,
    /// 未登録のサービス
    Unregistered,
}

/// ELF関連
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elf {
    /// 無効なELFフォーマット
    InvalidFormat,
    /// サポートされていないELFタイプ
    UnsupportedType,
    /// セグメントのロード失敗
    SegmentLoadFailure,
    /// シンボル解決失敗
    SymbolResolutionFailure,
    /// Elfファイルの長さ不足
    InsufficientLength,
}

/// デバイス関連
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    /// デバイスがビジー状態
    Busy,
    /// ハードウェアエラー
    HardwareFailure,
    /// タイムアウト
    Timeout,
    /// 不正な操作
    InvalidOperation,
    /// デバイスが見つからない
    DeviceNotFound,
    /// ドライバのロード失敗
    DriverLoadFailure,
    /// 切断されたデバイス
    Disconnected,
    /// サポートされていないデバイス
    Unsupported,
    /// 通信中に切断
    CommunicationLost,
    /// リソース不足
    ResourceUnavailable,
}

impl Kernel {
    /// このエラーが致命的かどうか
    ///
    /// 致命的なエラーは回復不能であり、システムの継続が不可能
    /// - `Memory::OutOfMemory`
    /// - `Device::HardwareFailure`
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            Kernel::Memory(Memory::OutOfMemory) | Kernel::Device(Device::HardwareFailure)
        )
    }

    /// このエラーがリトライ可能かどうか
    ///
    /// リトライ可能なエラーは、一時的な問題であり、再試行によって成功する可能性がある
    /// - `Device::Busy`
    /// - `Device::Timeout`
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Kernel::Device(Device::Busy) | Kernel::Device(Device::Timeout)
        )
    }
}

impl fmt::Display for Kernel {
    /// エラーをフォーマット表示
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kernel::Memory(e) => write!(f, "Memory error: {:?}", e),
            Kernel::Process(e) => write!(f, "Process error: {:?}", e),
            Kernel::Device(e) => write!(f, "Device error: {:?}", e),
            Kernel::Elf(e) => write!(f, "ELF error: {:?}", e),
            Kernel::InvalidParam => write!(f, "Invalid parameter"),
            Kernel::NotImplemented => write!(f, "Not implemented"),
        }
    }
}

impl fmt::Display for Memory {
    /// エラーをフォーマット表示
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Memory::OutOfMemory => write!(f, "Out of memory"),
            Memory::InvalidAddress => write!(f, "Invalid address"),
            Memory::PermissionDenied => write!(f, "Permission denied"),
            Memory::AlreadyMapped => write!(f, "Already mapped"),
            Memory::NotMapped => write!(f, "Not mapped"),
            Memory::AlignmentError => write!(f, "Alignment error"),
        }
    }
}

/// カーネルエラーを処理
pub fn handle_kernel_error(error: Kernel) {
    crate::warn!("KERNEL ERROR: {}", error);
    crate::debug!("Is fatal: {}", error.is_fatal());
    crate::debug!("Is retryable: {}", error.is_retryable());

    match error {
        Kernel::Memory(mem_err) => {
            crate::warn!("Memory error: {:?}", mem_err);
        }
        Kernel::Process(proc_err) => {
            crate::warn!("Process error: {:?}", proc_err);
        }
        Kernel::Device(dev_err) => {
            crate::warn!("Device error: {:?}", dev_err);
        }
        _ => {
            crate::warn!("Unknown error: {:?}", error);
        }
    }

    crate::info!("System halted.");
}

/// 結果型のエイリアス
pub type Result<T> = core::result::Result<T, Kernel>;
