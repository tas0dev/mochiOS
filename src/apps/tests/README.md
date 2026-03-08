# mochiOS Test Suite

このディレクトリには、mochiOSの各種機能テストが含まれています。

## ビルド方法

### テストを含めてビルド
```bash
START_TEST_APP=true cargo build
```

### テストなしでビルド（通常）
```bash
cargo build
```

## テストの追加方法

1. `src/main.rs`に新しいテスト関数を追加：
```rust
fn test_my_feature() -> bool {
    // テストロジック
    let result = my_function();
    result == expected_value
}
```

2. `main`関数内でテストを実行：
```rust
stats.run_test("my_feature\0", test_my_feature);
```

## 実行方法

core.serviceが自動的にテストアプリケーションを起動します。
`START_TEST_APP=true`でビルドした場合のみ実行されます。

## テストの種類

現在実装されているテスト：
- `test_basic_arithmetic` - 基本的な算術演算
- `test_string_compare` - 文字列比較
- `test_argc_argv` - コマンドライン引数の受け取り

将来的に追加予定：
- ファイルシステムテスト（read/write/seek）
- IPCテスト（プロセス間通信）
- メモリ管理テスト（alloc/free）
- システムコールテスト（各種syscall）
