#!/bin/bash

set -e

OVMF_PATHS=(
    "/usr/share/ovmf/OVMF.fd"
    "/usr/share/ovmf/x64/OVMF.fd"
    "/usr/share/edk2-ovmf/x64/OVMF.fd"
    "/usr/share/qemu/OVMF.fd"
    "/usr/share/OVMF/OVMF_CODE.fd"
)

OVMF=""
for path in "${OVMF_PATHS[@]}"; do
    if [ -f "$path" ]; then
        OVMF="$path"
        break
    fi
done

if [ -z "$OVMF" ]; then
    echo "Error: OVMF firmware not found. Please install ovmf package."
    echo "  Ubuntu: sudo apt install ovmf"
    echo "  Arch Linux: sudo pacman -S edk2-ovmf"
    exit 1
fi

SRC="$1"

if [ -z "$SRC" ]; then
    echo "Usage: $0 <EFI_FILE|BOOT_DIR>"
    exit 1
fi

if [ -d "$SRC" ]; then
    BOOT_DIR="$SRC"
    echo "Using directory as FAT root: $BOOT_DIR"
    DRIVE_ARG="fat:rw:$BOOT_DIR"
else
    if [ ! -f "$SRC" ]; then
        echo "Error: EFI file not found: $SRC"
        exit 1
    fi

    TEMP_DIR=$(mktemp -d)
    trap "rm -rf $TEMP_DIR" EXIT

    mkdir -p "$TEMP_DIR/esp/EFI/BOOT"
    cp "$SRC" "$TEMP_DIR/esp/EFI/BOOT/BOOTX64.EFI"
    DRIVE_ARG="fat:rw:$TEMP_DIR/esp"
fi

exec qemu-system-x86_64 \
    -bios "$OVMF" \
    -drive format=raw,file=${DRIVE_ARG} \
    -net none \
    -m 512M \
    -serial stdio \
    -vga std \
    -no-reboot \
    -d int,guest_errors \
    -D qemu.log