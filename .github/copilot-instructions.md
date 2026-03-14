# mochiOS Copilot インストラクション

mochiOS は Rust（no_std、nightly）で書かれたx86-64のハイブリッドアーキテクチャカーネルです。カーネルはハイブリッドカーネル設計を参考にしており、最小限のカーネルコアが単一の `core.service` を起動し、そこからディスク・ファイルシステム・シェルの各サービスを IPC 経由でユーザー空間に立ち上げます。
絶対にクラッシュしないことを目標にしています。

## ビルドと実行

```bash
# 初回セットアップ
git submodule update --init --recursive
cd src/lib && ./configure && cd ../..

# ビルド（UEFI ディスクイメージを生成）
cargo build

# ビルドして QEMU で実行
cargo run

# リリースビルド
cargo build --release
```

ビルドは `build.rs` が `builders/` 配下のモジュールを呼び出して制御します。カーネル ELF・newlib（libc）・サービス/ユーティリティバイナリのコンパイル、`ramfs/` を `initfs.ext2` へ、`fs/` を `rootfs.ext2` へパック、最終的な UEFI ESP イメージの組み立て、という流れです。
実行はユーザーが行います。あなたは実装だけ行うようにしてください。

### ブートフロー

```
UEFI ファームウェア
  └─ src/boot/loader.rs         # カーネル ELF + initfs/rootfs イメージをロード、BootInfo を構築
       └─ src/core/entry.rs     # カーネルエントリ（sysv64 ABI）
            └─ src/core/kernel.rs  # 初期化: メモリ → GDT/TSS → IDT/PIC → ヒープ → core.service 起動
                 └─ core.service → disk.service → fs.service → shell.service（IPC 管理）
```

### ソースレイアウト

| パス                | 役割                                               |
|-------------------|--------------------------------------------------|
| `src/boot/`       | UEFI ブートローダー — ELF ロード、メモリマップ、BootInfo           |
| `src/core/`       | カーネル — 割り込み、メモリ、スケジューラ、システムコール、ELF ローダー          |
| `src/services/`   | ユーザー空間サービス（core, disk, fs, shell）— 各々独立したクレート    |
| `src/user/`       | `swiftlib` — ユーザー空間バイナリ向けシステムコールラッパー、IPC、アロケータ   |
| `src/utils/`      | ユーザー空間ユーティリティ（ls, echo, cat, pwd）                |
| `src/posix/`      | POSIX 互換シム                                       |
| `src/lib/`        | Newlib（git サブモジュール）                              |
| `builders/`       | `build.rs` から呼び出されるビルドスクリプトモジュール                 |
| `ramfs/`          | initfs ソース（サービス ELF + 最小限のライブラリ、ビルド時に ext2 へパック） |
| `fs/`             | ルートファイルシステムソース（ビルド時に ext2 へパック）                  |

### カーネルサブシステム（`src/core/`）

- `mem/` — GDT、TSS、ページング、ヒープアロケータ（`linked_list_allocator`）
- `interrupt/` — IDT、PIC、タイマー、システムコールディスパッチ
- `task/` — プロセス・スレッドスケジューラ
- `syscall/` — exec、IPC、fs、I/O、時刻など
- `elf/` — ユーザー空間 ELF ローダー
- `percpu.rs` — CPU ごとの状態管理
- `panic.rs` — カーネルパニックハンドラ

### カスタムビルドターゲット

カーネルは `src/x86_64-mochios.json` 向けにコンパイルされます：
- LLVM トリプル：`x86_64-unknown-linux-musl`（ベアメタル向けカスタマイズ）
- MMX/AVX 無効、コードモデル：small、リロケーション：static
- リンカー：`rust-lld`、パニック戦略：abort、レッドゾーン無効
- カーネルは `0x200000` にリンク（`src/core/kernel.ld` 参照）

ブートローダーは `x86_64-unknown-uefi` ターゲットを使用します。

## 主な規約

### `no_std` の徹底
カーネルコード（`src/core/`、`src/boot/`）はすべて `#![no_std]` を使用します。カーネルやブートローダーに `std` のインポートや `std` 専用クレートを追加しないでください。

### カーネルコードで `unwrap()` 禁止
カーネルは `#![deny(clippy::unwrap_used)]` を設定しています。`result.rs` で定義された `Result<T>` エイリアス（`Result<T, KernelError>`）を使用してください。エラーバリアントは `result.rs` の `Kernel`・`Process`・`Memory` カテゴリに定義されています。

### サービスは独立したクレート
`src/services/<name>/` 配下の各サービスは、独自の `Cargo.toml` と `src/main.rs` を持ちます。サービスは `builders/services.rs` でコンパイルされます（ワークスペースではありません）。サービスのマニフェストは `src/services/index.toml` にあります。

### IPC がサービス間通信の唯一の手段
サービス間の通信は OP コードを使ったメッセージパッシングのみで行います。サービスをまたいだ直接の関数呼び出しは存在しません。

### アドレスは `u64`
仮想アドレスおよび物理アドレスは、カーネル全体を通じて `u64` で表現します（可能な限り `usize` や生ポインタは使わない）。

### Rust ツールチェーン
`rust-toolchain.toml` で定義：nightly チャンネル、コンポーネントは `rust-src` と `llvm-tools`。すべての `#![feature(...)]` 属性が引き続きコンパイルできることを確認せずにチャンネルを変更しないでください。

### ソースとドキュメント内の日本語
コメント、ドキュメントは日本語で記載されています。プロジェクトの README と CODE_OF_CONDUCT も日本語です。これは意図的なものです。 「修正」や翻訳はしないでください。