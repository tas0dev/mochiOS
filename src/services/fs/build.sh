#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_NAME=$(basename "$SCRIPT_DIR")
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

cd "$SCRIPT_DIR"

echo "Building user application: $APP_NAME"

export RUST_TARGET_PATH="$PROJECT_ROOT/src/lib"
export RUSTFLAGS="-C link-arg=-L$SCRIPT_DIR -C link-arg=-T$SCRIPT_DIR/linker.ld"

cargo build --release \
    --target="$RUST_TARGET_PATH/x86_64-mochios.json" \
    -Z build-std=core,alloc \
    --package "$APP_NAME"

INITFS_DIR="$PROJECT_ROOT/initfs"
mkdir -p "$INITFS_DIR"

SOURCE_BIN="target/x86_64-mochios/release/$APP_NAME"

if [ -f "$SOURCE_BIN" ]; then
    cp "$SOURCE_BIN" "$INITFS_DIR/$APP_NAME.elf"
    echo "Built successfully: $INITFS_DIR/$APP_NAME.elf"
    ls -lh "$INITFS_DIR/$APP_NAME.elf"
else
    echo "Error: Binary $SOURCE_BIN not found."
    exit 1
fi