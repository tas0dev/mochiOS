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

TEMP_DIR=$(mktemp -d)
# shellcheck disable=SC2064
trap "rm -rf $TEMP_DIR" EXIT

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

mkdir -p "$TEMP_DIR/esp/EFI/BOOT"
cp "$SRC" "$TEMP_DIR/esp/EFI/BOOT/BOOTX64.EFI"

# kernel.elf を ESP の \System\ に配置（ブートローダーが参照するパス）
mkdir -p "$TEMP_DIR/esp/System"
KERNEL_ELF="$ROOT_DIR/fs/System/kernel.elf"
if [ -f "$KERNEL_ELF" ]; then
    cp "$KERNEL_ELF" "$TEMP_DIR/esp/System/kernel.elf"
    echo "kernel.elf -> esp/System/kernel.elf"
else
    echo "Warning: kernel.elf not found at $KERNEL_ELF" >&2
    echo "  Run 'cargo build' first to build the kernel." >&2
fi

# initfs.ext2 を ESP の \System\initfs.img として配置
INITFS_IMG=$(find "$ROOT_DIR/target/x86_64-unknown-uefi" -name "initfs.ext2" -not -path "*/kernel/*" 2>/dev/null | sort -t/ -k1,1 | tail -1)
if [ -n "$INITFS_IMG" ] && [ -f "$INITFS_IMG" ]; then
    cp "$INITFS_IMG" "$TEMP_DIR/esp/System/initfs.img"
    echo "initfs.ext2 -> esp/System/initfs.img"
else
    echo "Warning: initfs.ext2 not found" >&2
fi

exec qemu-system-x86_64 \
    -bios "$OVMF" \
    -drive format=raw,file=fat:rw:"$TEMP_DIR/esp" \
    -drive id=disk0,file=target/swiftCore.img,format=raw,if=ide,index=1,media=disk \
    -net none \
    -m 512M \
    -no-reboot \
    -d int,guest_errors \
    -D qemu.log \