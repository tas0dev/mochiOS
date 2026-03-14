#!/bin/bash
set -e

# プロジェクトルートへ移動
cd "$(dirname "$0")/.."

# initfsディレクトリの場所
INITFS_DIR="fs"
OUTPUT_IMG="target/mochiOS.img"
SIZE="256M"

echo "Creating disk image: $OUTPUT_IMG (Source: $INITFS_DIR)"

# ターゲットディレクトリがない場合は作成
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
# -L mochiOS: ボリュームラベル
# -F: ファイルへの書き込みを強制
mke2fs -t ext2 -b 4096 -d "$INITFS_DIR" -L mochiOS -F "$OUTPUT_IMG" "$SIZE"

echo "Done."
