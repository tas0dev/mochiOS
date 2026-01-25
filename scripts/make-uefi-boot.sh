#!/usr/bin/env bash
set -euo pipefail

CFG=${1:-debug}
ROOT_DIR=$(pwd)
TARGET_DIR="$ROOT_DIR/target/x86_64-unknown-uefi/$CFG/boot"

echo "Assembling UEFI boot directory: $TARGET_DIR"

mkdir -p "$TARGET_DIR/EFI/BOOT"
mkdir -p "$TARGET_DIR/services"

if [ -d "$ROOT_DIR/src/initfs" ]; then
    echo "Copying initfs files..."
    cp -r "$ROOT_DIR/src/initfs/." "$TARGET_DIR/"
fi

EFI_CANDIDATES=(
    "$ROOT_DIR/target/x86_64-unknown-uefi/$CFG/boot/BOOTX64.EFI"
    "$ROOT_DIR/target/x86_64-unknown-uefi/$CFG/boot/boot.efi"
    "$ROOT_DIR/target/x86_64-unknown-uefi/$CFG/boot/boot"
)

FOUND_EFI=""
for p in "${EFI_CANDIDATES[@]}"; do
    if [ -f "$p" ]; then
        FOUND_EFI="$p"
        break
    fi
done

if [ -z "$FOUND_EFI" ]; then
    echo "Searching for EFI binary in target directory..."
    FOUND_EFI=$(find "$ROOT_DIR/target" -type f \( -iname "*.efi" -o -iname "boot" -o -iname "bootx64*" \) | head -n1 || true)
fi

if [ -n "$FOUND_EFI" ]; then
    echo "Found EFI binary: $FOUND_EFI"
    cp "$FOUND_EFI" "$TARGET_DIR/EFI/BOOT/BOOTX64.EFI"
else
    echo "Warning: No EFI binary found in target. Build the project first." >&2
fi

if [ -d "$ROOT_DIR/src/services" ]; then
    for svc in "$ROOT_DIR/src/services"/*; do
        if [ -d "$svc" ]; then
            name=$(basename "$svc")
            # Create a placeholder descriptor if none exists
            echo "name=$name" > "$TARGET_DIR/services/$name.service"
        fi
    done
fi

echo "Boot directory assembled at: $TARGET_DIR"
