//! SwiftCoreエラー型定義
//!
//! すべてのカーネルエラーをResult型で表現し、panicを禁止

use core::fmt;

/// トップレベルエラー型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelError {
    /// メモリエラー
    Memory(MemoryError),
    /// プロセスエラー
    Process(ProcessError),
    /// デバイスエラー
    Device(DeviceError),
    /// 無効なパラメータ
    InvalidParam,
    /// 未実装の機能
    NotImplemented,
}

/// メモリ関連のエラー
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
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

/// プロセス関連のエラー
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
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
    Service(ServiceError),
}

/// サービス関連のエラー
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceError {
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

/// デバイス関連のエラー
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceError {
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
}

impl KernelError {
    /// このエラーが致命的かどうか
    ///
    /// 致命的なエラーは回復不能であり、システムの継続が不可能
    /// - `MemoryError::OutOfMemory`
    /// - `DeviceError::HardwareFailure`
    pub fn is_fatal(&self) -> bool {
        match self {
            KernelError::Memory(MemoryError::OutOfMemory) => true,
            KernelError::Device(DeviceError::HardwareFailure) => true,
            _ => false,
        }
    }

    /// このエラーがリトライ可能かどうか
    ///
    /// リトライ可能なエラーは、一時的な問題であり、再試行によって成功する可能性がある
    /// - `DeviceError::Busy`
    /// - `DeviceError::Timeout`
    pub fn is_retryable(&self) -> bool {
        match self {
            KernelError::Device(DeviceError::Busy) => true,
            KernelError::Device(DeviceError::Timeout) => true,
            _ => false,
        }
    }
}

impl fmt::Display for KernelError {
    /// エラーをフォーマット表示
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KernelError::Memory(e) => write!(f, "Memory error: {:?}", e),
            KernelError::Process(e) => write!(f, "Process error: {:?}", e),
            KernelError::Device(e) => write!(f, "Device error: {:?}", e),
            KernelError::InvalidParam => write!(f, "Invalid parameter"),
            KernelError::NotImplemented => write!(f, "Not implemented"),
        }
    }
}

impl fmt::Display for MemoryError {
    /// エラーをフォーマット表示
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryError::OutOfMemory => write!(f, "Out of memory"),
            MemoryError::InvalidAddress => write!(f, "Invalid address"),
            MemoryError::PermissionDenied => write!(f, "Permission denied"),
            MemoryError::AlreadyMapped => write!(f, "Already mapped"),
            MemoryError::NotMapped => write!(f, "Not mapped"),
            MemoryError::AlignmentError => write!(f, "Alignment error"),
        }
    }
}

/// 結果型のエイリアス
pub type Result<T> = core::result::Result<T, KernelError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_is_fatal() {
        assert!(KernelError::Memory(MemoryError::OutOfMemory).is_fatal());
        assert!(!KernelError::Memory(MemoryError::InvalidAddress).is_fatal());
    }

    #[test]
    fn test_error_is_retryable() {
        assert!(KernelError::Device(DeviceError::Busy).is_retryable());
        assert!(!KernelError::Memory(MemoryError::OutOfMemory).is_retryable());
    }
}
