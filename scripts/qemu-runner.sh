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

EFI_FILE="$1"

if [ ! -f "$EFI_FILE" ]; then
    echo "Error: EFI file not found: $EFI_FILE"
    exit 1
fi

TEMP_DIR=$(mktemp -d)
# shellcheck disable=SC2064
trap "rm -rf $TEMP_DIR" EXIT

mkdir -p "$TEMP_DIR/esp/EFI/BOOT"

cp "$EFI_FILE" "$TEMP_DIR/esp/EFI/BOOT/BOOTX64.EFI"

exec qemu-system-x86_64 \
    -bios "$OVMF" \
    -drive format=raw,file=fat:rw:"$TEMP_DIR/esp" \
    -drive id=disk0,file=target/swiftCore.img,format=raw,if=ide,index=0,media=disk \
    -net none \
    -m 512M \
    -serial stdio \
    -vga std \
    -no-reboot \
    -d int,guest_errors \
    -D qemu.log