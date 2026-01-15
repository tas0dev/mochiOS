## 基本設計方針

**絶対panicしないカーネル**
カーネル空間でのpanicはシステム全体の停止を意味します。SwiftCoreでは`panic!`を完全に禁止し、すべてのエラーを`Result`型で表現します。

**エラー型の階層化**
すべてのカーネルエラーを`KernelError`列挙型に集約しますが、サブシステムごとに詳細なエラー型も用意します。

```rust
// トップレベルエラー型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelError {
    Memory(MemoryError),
    Process(ProcessError),
    Fs(FileSystemError),
    Device(DeviceError),
    Ipc(IpcError),
    InvalidParam,
    NotImplemented,
}

// サブシステムごとの詳細エラー
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    OutOfMemory,
    InvalidAddress,
    PermissionDenied,
    AlreadyMapped,
    NotMapped,
    AlignmentError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    InvalidPid,
    ProcessNotFound,
    ZombieProcess,
    MaxProcessesReached,
    InsufficientPrivilege,
}

// 他のサブシステムも同様...
```

## エラーハンドリングの原則

**原則1: すべてのエラーに回復パスを定義**
各エラーに対して以下を明確にします:
- **即座に回復可能**: リトライや代替手段で処理継続
- **呼び出し側に伝播**: 上位レイヤーでの判断が必要
- **システムログ後に継続**: エラーを記録して処理継続
- **プロセス終了**: 当該プロセスのみ終了
- **致命的エラー**: システム停止(最終手段)

```rust
impl KernelError {
    /// このエラーが致命的かどうか
    pub fn is_fatal(&self) -> bool {
        match self {
            KernelError::Memory(MemoryError::OutOfMemory) => true,
            KernelError::Device(DeviceError::HardwareFailure) => true,
            _ => false,
        }
    }
    
    /// このエラーがリトライ可能かどうか
    pub fn is_retryable(&self) -> bool {
        match self {
            KernelError::Ipc(IpcError::BufferFull) => true,
            KernelError::Device(DeviceError::Busy) => true,
            _ => false,
        }
    }
}
```

**原則2: matchによる網羅的処理の強制**
エラーハンドリングでワイルドカードパターン(`_`)の使用を禁止します。これによりコンパイラが全パターンチェックを強制します。

```rust
// 良い例: すべてのエラーを明示的に処理
match allocate_page() {
    Ok(page) => use_page(page),
    Err(KernelError::Memory(MemoryError::OutOfMemory)) => {
        // OOMキラーを起動
        reclaim_memory_and_retry()
    },
    Err(KernelError::Memory(MemoryError::InvalidAddress)) => {
        log_error("Invalid address in allocation");
        return Err(KernelError::InvalidParam);
    },
    Err(e) => {
        // 他のエラーは上位に伝播
        return Err(e);
    }
}

// 悪い例: ワイルドカードで隠蔽(Lintで禁止)
match allocate_page() {
    Ok(page) => use_page(page),
    Err(_) => return Err(KernelError::Memory(MemoryError::OutOfMemory)), // NG
}
```

**原則3: エラーコンテキストの保持**
エラーが発生した場所とコンテキストを追跡できるようにします。

```rust
// エラーにコンテキストを追加
pub struct ErrorContext {
    pub error: KernelError,
    pub file: &'static str,
    pub line: u32,
    pub function: &'static str,
}

macro_rules! kernel_error {
    ($err:expr) => {
        ErrorContext {
            error: $err,
            file: file!(),
            line: line!(),
            function: core::any::type_name::<fn()>(),
        }
    };
}

// 使用例
fn allocate_page() -> Result<Page, ErrorContext> {
    if no_memory_available() {
        return Err(kernel_error!(
            KernelError::Memory(MemoryError::OutOfMemory)
        ));
    }
    // ...
}
```

## 回復戦略の実装

**戦略1: リトライメカニズム**
リトライ可能なエラーに対する統一的な再試行機構を提供します。

```rust
pub fn retry_with_backoff<T, F>(
    mut f: F,
    max_attempts: usize,
) -> Result<T, KernelError>
where
    F: FnMut() -> Result<T, KernelError>,
{
    for attempt in 0..max_attempts {
        match f() {
            Ok(val) => return Ok(val),
            Err(e) if e.is_retryable() => {
                // 指数バックオフ
                sleep_ticks(1 << attempt);
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(KernelError::Device(DeviceError::Timeout))
}

// 使用例
retry_with_backoff(|| send_ipc_message(msg), 3)?;
```

**戦略2: フォールバック処理**
主要な処理が失敗した場合の代替手段を型で表現します。

```rust
pub enum AllocationStrategy {
    Primary,
    Fallback,
    Emergency,
}

pub fn allocate_with_fallback() -> Result<Page, KernelError> {
    // 通常のアロケーション
    if let Ok(page) = allocate_from_pool(AllocationStrategy::Primary) {
        return Ok(page);
    }
    
    // フォールバック: スワップアウト
    if let Ok(page) = swap_out_and_allocate() {
        log_warn("Used swap for allocation");
        return Ok(page);
    }
    
    // 緊急: プロセス終了してメモリ回収
    if let Ok(page) = kill_low_priority_and_allocate() {
        log_warn("Killed process for memory");
        return Ok(page);
    }
    
    Err(KernelError::Memory(MemoryError::OutOfMemory))
}
```

**戦略3: タイプステートパターンでの状態管理**
エラー後の状態を型で表現し、無効な操作をコンパイル時に防ぎます。

```rust
// ファイルの状態を型で表現
pub struct File<State> {
    fd: FileDescriptor,
    _state: PhantomData<State>,
}

pub struct Open;
pub struct Closed;
pub struct Error;

impl File<Open> {
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, KernelError> {
        // 読み込み処理
    }
    
    pub fn close(self) -> File<Closed> {
        // クローズ処理
        File {
            fd: self.fd,
            _state: PhantomData,
        }
    }
}

impl File<Closed> {
    pub fn reopen(self) -> Result<File<Open>, (File<Error>, KernelError)> {
        match try_reopen(self.fd) {
            Ok(_) => Ok(File {
                fd: self.fd,
                _state: PhantomData,
            }),
            Err(e) => Err((File {
                fd: self.fd,
                _state: PhantomData,
            }, e)),
        }
    }
}

// File<Closed>にはreadメソッドがないので、
// クローズ後の読み込みはコンパイルエラー
```

## テスト戦略

**単体テスト: エラーパスの網羅**
すべてのエラーケースに対するテストを書きます。

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_out_of_memory_handling() {
        // メモリを使い切る
        let _pages = exhaust_memory();
        
        match allocate_page() {
            Err(KernelError::Memory(MemoryError::OutOfMemory)) => {
                // 期待通り
            },
            _ => panic!("Expected OutOfMemory error"),
        }
    }
    
    #[test]
    fn test_error_recovery() {
        simulate_low_memory();
        
        // フォールバックが動作することを確認
        let page = allocate_with_fallback().expect("Should recover");
        assert!(page.is_valid());
    }
}
```

**統合テスト: エラー注入**
実行時にエラーを注入してシステムの挙動を確認します。

```rust
#[cfg(feature = "error_injection")]
pub mod error_injection {
    use core::sync::atomic::{AtomicBool, Ordering};
    
    static INJECT_OOM: AtomicBool = AtomicBool::new(false);
    
    pub fn enable_oom_injection() {
        INJECT_OOM.store(true, Ordering::SeqCst);
    }
    
    pub fn should_inject_oom() -> bool {
        INJECT_OOM.load(Ordering::SeqCst)
    }
}

// アロケータ内で使用
fn allocate_page_impl() -> Result<Page, KernelError> {
    #[cfg(feature = "error_injection")]
    if error_injection::should_inject_oom() {
        return Err(KernelError::Memory(MemoryError::OutOfMemory));
    }
    
    // 通常のアロケーション処理
}
```

**Fuzzing: 異常系の発見**
cargo-fuzzを使ってシステムコールの異常系をテストします。

```rust
#[cfg(fuzzing)]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    
    let syscall_num = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let param = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    
    // システムコールを実行してpanicしないことを確認
    let _ = execute_syscall(syscall_num, param);
});
```

## Lintルールの設定

**カスタムLintでポリシーを強制**
Clippyのカスタムルールで、エラーハンドリングポリシーを強制します。

```rust
// .cargo/config.toml または rust-toolchain.toml で設定
// [lints.clippy]
// unwrap_used = "deny"
// expect_used = "deny"
// panic = "deny"
// wildcard_enum_match_arm = "deny"  // match文での _ 禁止

// コード内での設定
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::wildcard_enum_match_arm)]
```

## ドキュメント化

**各エラーの回復手順を文書化**
エラー型の定義に回復戦略をドキュメントとして含めます。

```rust
/// メモリ関連のエラー
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    /// 利用可能なメモリがない
    /// 
    /// # 回復戦略
    /// 1. スワップアウトを試みる
    /// 2. キャッシュをクリアする
    /// 3. 低優先度プロセスを終了する
    /// 4. すべて失敗した場合、呼び出し側にエラーを返す
    /// 
    /// # 致命度
    /// システム全体で回復不能な場合のみ致命的
    OutOfMemory,
    
    /// 無効なアドレスへのアクセス
    /// 
    /// # 回復戦略
    /// プロセスにSIGSEGVを送信してプロセスを終了
    /// 
    /// # 致命度
    /// 非致命的（プロセスレベルで処理）
    InvalidAddress,
    
    // ...
}
```


## どうしてもクラッシュしてしまう場合
カーネルをどうしてもクラッシュさせなければならない場合は、別のカーネルイメージへの高速再起動やスタンバイ系カーネルへの切り替えなどにより、新しいカーネルへ制御を移譲して復旧を試みます。
また、ユーザーにクラッシュしたことを認識させないために、できる限りこの処理は迅速に行う必要があります。
そして、クラッシュ判定時点でのユーザープロセスのアドレス空間（ヒープ・スタック・メモリマップトファイルなど）およびページキャッシュなどの「ユーザ空間データ」を、カーネル自身のスタックやカーネルヒープを除いて可能な限り保持するべきです。具体的には、(1) 旧カーネルが管理していたユーザ空間用ページテーブルと物理ページフレームを新カーネルインスタンスに引き継ぐ、または (2) クラッシュ検知時にユーザープロセス状態を永続メモリ／ストレージにスナップショット（シリアライズ）し、新カーネル起動後に再マッピング・復元する、といったメカニズムを用いることを想定します。
ユーザープロセスの終了方針については、セキュリティおよびシステム安定性を最優先とし、以下のように区別します。
- 悪意のある挙動が疑われるプロセス、権限昇格・メモリ/デバイス不正アクセスなどのセキュリティ侵害が疑われるプロセス、
  またはシステム全体のリソースを過度に消費して他プロセスに影響を与えている暴走プロセスについては、ユーザーの許可を待たずに即座に終了させます。
- 上記に該当せず、主に当該プロセス自身の状態やユーザーデータの損失のみが問題となる通常のユーザーアプリケーションについては、
  可能なかぎりユーザーと対話し、明示的な許可を得たうえで終了させます（ただし、対話が不可能または著しく遅延する場合は安全側に倒して終了します）。
新たなカーネルプロセスを起動することも不可能（デバイス由来の問題など）な場合はクラッシュします。

つまり、クラッシュは最終手段であり、SwiftCoreはこれをできるだけ避けるべきです。
