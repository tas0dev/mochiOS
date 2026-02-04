#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "Building user application"

cargo build --release --target=x86_64-swiftcore.json

cp target/x86_64-swiftcore/release/test_app ../../initfs/test.elf

echo "Built successfully"
ls -lh ../../initfs/test.elf
