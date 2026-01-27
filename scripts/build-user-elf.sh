#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
INITFS_DIR="$ROOT_DIR/src/initfs"
ASM="$INITFS_DIR/hello.asm"
OUT="$INITFS_DIR/hello"

# Use nasm + ld by default
NASM=${1:-nasm}
LD=${2:-ld}

if ! command -v "$NASM" >/dev/null 2>&1; then
    echo "Error: nasm not found. Install nasm or pass assembler as first arg."
    exit 1
fi
if ! command -v "$LD" >/dev/null 2>&1; then
    echo "Error: ld not found. Install binutils or pass linker as second arg."
    exit 1
fi

echo "Assembling with: $NASM; linking with: $LD"

TMPOBJ="$OUT.o"
$NASM -f elf64 -o "$TMPOBJ" "$ASM"
$LD -static -o "$OUT" "$TMPOBJ"
rm -f "$TMPOBJ"

echo "Built: $OUT"
