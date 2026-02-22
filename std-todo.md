# Rust std を動かすための TODO

SwiftCore 上で Rust の `std` を動かすために必要な作業と、現在の実装の問題点をまとめる。

---

## 🐛 現在のバグ（今すぐ直すべき）

### 1. `src/libc` パスが存在しない
- **場所**: `Cargo.toml` の `[patch.crates-io]`
- **問題**: `libc = { path = "src/libc" }` となっているが、実際のディレクトリは `src/libc-rs`
- **修正**: パスを `src/libc-rs` に修正するか、ディレクトリ名を `src/libc` にリネームする

### 2. `libc-rs/Cargo.toml` のクレート名が `libc` でない
- **場所**: `src/libc-rs/Cargo.toml`
- **問題**: `name = "libc-rs"` になっており、`[patch.crates-io]` の置き換えが機能しない（Cargoはクレート名で照合する）
- **補足**: `src/libc-rs/cargo.toml`（小文字）には `name = "libc"` と書かれているが、Cargoは大文字の `Cargo.toml` しか読まない
- **修正**: `src/libc-rs/Cargo.toml` の `name` を `"libc"` に変更する

### 3. `libc-rs/src/lib.rs` が `swiftlib` に依存しているが依存関係に書いていない
- **場所**: `src/libc-rs/src/lib.rs` の `pub use swiftlib::*;`
- **問題**: `libc-rs/Cargo.toml` に `swiftlib` の依存関係がないためビルドエラーになる
- **修正**: `[dependencies]` に `swiftlib = { path = "../user" }` を追加するか、`libc-rs` の内容をスタンドアロンに書き直す

### 4. `libc-rs/src/lib.rs` に `#![no_main]` がついている
- **場所**: `src/libc-rs/src/lib.rs`
- **問題**: ライブラリクレートに `#![no_main]` は意味がない（`no_main` はバイナリ用の属性）
- **修正**: `#![no_main]` を削除する

### 5. `swiftlib` に `cfunc` モジュールがない
- **場所**: `src/apps/tests/src/main.rs` と `src/services/disk/src/ata.rs`
- **問題**: `use swiftlib::cfunc::*;` や `use swiftlib::cfunc::{inb, outb, inw, outw}` を使っているが、`src/user/` に `cfunc.rs` が存在しない
- **修正**: `src/user/cfunc.rs` を作成し、`lib.rs` から `pub mod cfunc;` で公開する

### 6. `memalign` が常に失敗する
- **場所**: `src/user/libc.rs` の `memalign` 関数
- **問題**: `return -1isize as *mut u8;` とハードコードされており、どんなメモリ確保も必ず失敗する。`GlobalAllocator` がこれを呼んでいるため、`alloc` クレート（`Vec`, `Box` 等）が一切使えない
- **修正**: `brk` システムコールを使ったヒープアロケータを実装するか（後述の sbrk ベースのアロケータ）、`mmap` を実装してそちらを使う

### 7. エラーコードが POSIX に従っていない
- **場所**: `src/core/syscall/types.rs`
- **問題**: `ENOSYS = u64::MAX`, `EINVAL = u64::MAX - 1` など独自の値になっている。POSIX/Linux の慣習では syscall は失敗時に `-1` を返し、`errno` に具体的なエラー番号（`ENOSYS = 38`, `EINVAL = 22` 等）をセットする
- **結果**: newlib や libc が期待するエラー処理と互換性がなく、std や libc の関数が正しくエラーを認識できない
- **修正**: syscall の戻り値規約を Linux に合わせ、失敗時は `(-errno as u64)` を返す（または errno を別途管理する仕組みを設ける）

---

## ⚠️ std を動かすために必要な実装

### 8. ターゲット仕様の変更
- **場所**: `src/x86_64-swiftcore.json`
- **問題**: `"os": "none"` になっているため、`-Z build-std` で std をビルドしても OS 固有のコードが一切使われない。`std` は OS を認識してシステムコールを呼べる必要がある
- **修正案A**: `"os": "linux"` に変更し、Linux システムコール番号に合わせた実装を行う（std の Linux PAL がそのまま使える）
- **修正案B**: `"os": "swiftcore"` にして Rust の std に SwiftCore 用の PAL を追加する（工数大）
- **推奨**: まず修正案A（Linux互換）で進める。syscall 番号を Linux に合わせ、`build-std` で std を動かす

### 9. Linux 互換 syscall 番号への変更
- **場所**: `src/core/syscall/types.rs` と `src/user/sys.rs`
- **問題**: 現在の syscall 番号は独自定義（`Write=7`, `Read=8` 等）。std が Linux ターゲットとしてビルドされた場合、Linux の syscall 番号（`write=1`, `read=0`, `exit=60`/`exit_group=231` 等）が使われる
- **修正**: syscall 番号を Linux x86_64 の番号に合わせるか、syscall ディスパッチで Linux 番号を受け付けるようにする

  | syscall   | 現在の番号 | Linux x86_64 番号 |
  |-----------|-----------|------------------|
  | read      | 8         | 0                |
  | write     | 7         | 1                |
  | open      | 12        | 2                |
  | close     | 13        | 3                |
  | fstat     | 18        | 5                |
  | lseek     | 17        | 8                |
  | mmap      | なし      | 9                |
  | munmap    | なし      | 11               |
  | brk       | 16        | 12               |
  | exit      | 6         | 60               |
  | getpid    | 9         | 39               |
  | clone     | なし      | 56               |
  | futex     | なし      | 202              |
  | exit_group| なし      | 231              |
  | clock_gettime | なし  | 228              |
  | nanosleep | なし      | 35               |
  | getcwd    | なし      | 79               |

### 10. `mmap` / `munmap` の実装
- **場所**: カーネル側 `src/core/syscall/` および `src/user/sys.rs`
- **問題**: Rust の global allocator（std の `System` allocator）は大きなメモリ確保に `mmap` を使う。現在未実装
- **修正**: 匿名 `mmap`（`MAP_ANONYMOUS | MAP_PRIVATE`）を実装する。プロセスのページテーブルに新しい物理フレームをマップして仮想アドレスを返す

### 11. スレッド生成（`clone` syscall）の実装
- **場所**: カーネル側 `src/core/syscall/`
- **問題**: `std::thread::spawn` は Linux では `clone(2)` syscall を使う。現在 `fork` は `ENOSYS` を返すのみで、`clone` は存在しない
- **修正**: `clone` syscall を実装する（最低限: `CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD` フラグのサポート。つまりスレッド作成）

### 12. `futex` syscall の実装
- **場所**: カーネル側 `src/core/syscall/`
- **問題**: std の `Mutex`, `Condvar`, `RwLock` は Linux では `futex(2)` に依存する
- **修正**: `FUTEX_WAIT` と `FUTEX_WAKE` の最低限の実装を追加する

### 13. TLS（スレッドローカルストレージ）の実装
- **場所**: カーネル側のスレッド管理、ELF ローダー
- **問題**: Rust std は `thread_local!` マクロや内部状態管理に TLS を使う。x86_64 では FS ベースレジスタ（`fs:0`）を使った実装が標準
- **修正**:
  1. ELF の `PT_TLS` セグメントをパース・マップする
  2. 各スレッド生成時に TLS テンプレートをコピーしてスレッド固有の TLS ブロックを確保する
  3. コンテキストスイッチ時に `WRFSBASE` 命令で FS ベースを切り替える
  4. `arch_prctl(ARCH_SET_FS, ...)` syscall（Linux 番号: 158）を実装して、ユーザー空間から FS ベースを設定できるようにする

### 14. `errno` のスレッドローカル管理
- **場所**: `src/user/` の syscall スタブ, `src/libc-rs/`
- **問題**: POSIX では errno はスレッドローカル変数。TLS が動く前は errno を正しく管理できない
- **修正**: TLS 実装後に `__errno_location()` をスレッドローカルな `errno` へのポインタを返すよう実装する

### 15. `clock_gettime` の実装
- **場所**: カーネル側 `src/core/syscall/time.rs`
- **問題**: std の `Instant::now()` や `SystemTime::now()` が使うため必須
- **修正**: `CLOCK_MONOTONIC` と `CLOCK_REALTIME` を実装する。HPET や TSC を使った時刻取得が必要

### 16. `nanosleep` の実装
- **場所**: カーネル側 `src/core/syscall/`
- **問題**: `std::thread::sleep` は `nanosleep(2)` を使う。現在は単純に yield するだけで精度がない
- **修正**: タイマー割り込みと組み合わせた sleep キューを実装する

### 17. シグナル処理の最低限の実装
- **場所**: カーネル側
- **問題**: std の panic ハンドラが `SIGABRT` を送る。また、スタックオーバーフロー検出（`SIGSEGV`）も std が処理する
- **修正**: 最低限 `SIGABRT` と `SIGSEGV` のデフォルト動作（プロセス終了）を実装する。`sigaction(2)` の最低限の実装も必要

### 18. `getcwd` の実装
- **場所**: カーネル側 `src/core/syscall/fs.rs`
- **問題**: std の `std::env::current_dir()` が使う
- **修正**: プロセスの現在ディレクトリを管理する構造体を追加し、`getcwd` syscall を実装する

### 19. ファイルシステム syscall の実装
- **場所**: `src/core/syscall/fs.rs`
- **問題**: `open`, `close`, `read`, `write`（fd 3以上）, `fstat`, `lseek` がすべて `ENOSYS` を返す。std の `File` が使えない
- **修正**: fs サービスと接続した VFS 層を実装し、実際にファイルを開いて読み書きできるようにする

---

## 📋 実装の推奨順序

1. **バグ修正**（1〜7）: まずビルドが通るようにする
2. **ターゲット変更**（8, 9）: `"os": "linux"` に変更し、syscall 番号を Linux に合わせる
3. **メモリ管理**（6, 10）: `memalign` を `sbrk` で実装し、`mmap` を追加する → `alloc` が動く
4. **TLS + errno**（13, 14）: TLS を実装する → `thread_local!` と errno が動く
5. **`-Z build-std` で std のビルドを試みる**: ここまでできれば基本的な std（I/O なし）が動くはず
6. **I/O**（7, 9, 19）: write/read syscall の errno 対応と fd 管理を実装 → `println!` が動く
7. **時刻**（15, 16）: `clock_gettime` と `nanosleep` → `Instant`, `thread::sleep` が動く
8. **スレッド**（11, 12）: `clone` と `futex` → `std::thread::spawn` が動く
9. **シグナル**（17）: panic 時の SIGABRT 処理 → panic が正しく動く
10. **ファイルシステム**（18, 19）: `File::open` 等が動く

---

## 📝 備考

- `build-std` を使うには `Cargo.toml` または `.cargo/config.toml` に以下を追加する:
  ```toml
  [unstable]
  build-std = ["std", "panic_abort"]
  build-std-features = ["panic_immediate_abort"]
  ```
- `src/libc-rs` の `extern crate rustc_std_workspace_core as core;` は、Rust の std 自体をビルドする際のリポジトリ内専用の特殊な記述。通常の `[patch.crates-io]` での `libc` 差し替えには不要なため、削除する
- カーネル側（`x86_64-unknown-uefi`）は MS x64 ABI、ユーザー側（`x86_64-swiftcore`）は System V AMD64 ABI を使う。コンテキストスイッチの `switch_context` は MS ABI で正しく実装されているが、混同しないよう注意
