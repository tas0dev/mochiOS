#!/bin/bash

set -e

echo "===== SwiftCore ELF Execution Test ====="
echo ""

# 1. ユーザーアプリをビルド
echo "[1/4] Building user application..."
cd src/user/test_app
./build.sh
cd ../../..

# 2. initfsの内容確認
echo ""
echo "[2/4] Checking initfs contents..."
ls -lh src/initfs/

# 3. test.elfのELFヘッダ確認
echo ""
echo "[3/4] Verifying ELF header..."
file src/initfs/test.elf
readelf -h src/initfs/test.elf | grep "Entry point"

# 4. カーネルをビルド
echo ""
echo "[4/4] Building kernel..."
cargo build

echo ""
echo "===== Build Complete ====="
echo "Run 'cargo run' to execute the kernel with test.elf"
