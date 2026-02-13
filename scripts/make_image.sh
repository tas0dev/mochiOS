#!/bin/bash
set -e

# プロジェクトルートへ移動
cd "$(dirname "$0")/.."

# initfsディレクトリの場所
INITFS_DIR="fs"
OUTPUT_IMG="target/swiftCore.img"
SIZE="128M"

echo "Creating disk image: $OUTPUT_IMG (Source: $INITFS_DIR)"

# ターゲットディレクトリがない場合は作成
# shellcheck disable=SC2046
mkdir -p $(dirname "$OUTPUT_IMG")

# mke2fsが利用可能か確認
if ! command -v mke2fs &> /dev/null; then
    echo "Error: mke2fs not found. Please install e2fsprogs."
    exit 1
fi

# イメージ作成とディレクトリ内容のコピー
# -t ext2: ファイルシステムタイプ
# -b 4096: ブロックサイズ
# -d $INITFS_DIR: ディレクトリ内容をルートにコピー
# -L swiftCore: ボリュームラベル
# -F: ファイルへの書き込みを強制
mke2fs -t ext2 -b 4096 -d "$INITFS_DIR" -L swiftCore -F "$OUTPUT_IMG" "$SIZE"

echo "Done."

