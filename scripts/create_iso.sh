#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

BOOT_SRC="${1:-}"
OUTPUT_ISO="${2:-$ROOT_DIR/target/mochiOS.iso}"

require_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Error: required command not found: $1" >&2
        exit 1
    fi
}

require_cmd mkdosfs
require_cmd mmd
require_cmd mcopy
require_cmd xorriso

if [ -z "$BOOT_SRC" ]; then
    BOOT_SRC=$(find "$ROOT_DIR/target/x86_64-unknown-uefi" -type f \( -name "BOOTX64.EFI" -o -name "boot.efi" -o -name "boot" \) -not -path "*/kernel/*" 2>/dev/null | xargs ls -t 2>/dev/null | head -1 || true)
fi

if [ -z "${BOOT_SRC:-}" ] || [ ! -f "$BOOT_SRC" ]; then
    echo "Error: UEFI boot binary not found." >&2
    echo "Usage: $0 [EFI_FILE] [OUTPUT_ISO]" >&2
    echo "Hint: run 'cargo build' first, or pass BOOTX64.EFI explicitly." >&2
    exit 1
fi

PROFILE="debug"
case "$BOOT_SRC" in
    */release/*) PROFILE="release" ;;
esac

FALLBACK_KERNEL="$ROOT_DIR/target/kernel/x86_64-unknown-none/$PROFILE/kernel"
FS_KERNEL="$ROOT_DIR/fs/System/kernel.elf"
if [ -f "$FALLBACK_KERNEL" ]; then
    KERNEL_ELF="$FALLBACK_KERNEL"
elif [ -f "$FS_KERNEL" ]; then
    KERNEL_ELF="$FS_KERNEL"
else
    echo "Error: kernel.elf not found." >&2
    echo "  Missing: $FALLBACK_KERNEL" >&2
    echo "  Missing: $FS_KERNEL" >&2
    echo "  Run 'cargo build' first to build the kernel." >&2
    exit 1
fi

INITFS_IMG=$(find "$ROOT_DIR/target/x86_64-unknown-uefi" -name "initfs.ext2" -not -path "*/kernel/*" 2>/dev/null | xargs ls -t 2>/dev/null | head -1 || true)
ROOTFS_IMG=$(find "$ROOT_DIR/target/x86_64-unknown-uefi" -name "rootfs.ext2" -not -path "*/kernel/*" 2>/dev/null | xargs ls -t 2>/dev/null | head -1 || true)

mkdir -p "$(dirname "$OUTPUT_ISO")"

TEMP_DIR=$(mktemp -d)
# shellcheck disable=SC2064
trap "rm -rf $TEMP_DIR" EXIT

EFI_IMG="$TEMP_DIR/efiboot.img"
ISO_ROOT="$TEMP_DIR/isoroot"
EFI_DIR="$ISO_ROOT/EFI/BOOT"

mkdir -p "$EFI_DIR"
cp "$BOOT_SRC" "$EFI_DIR/BOOTX64.EFI"

esp_bytes=$(stat -c%s "$BOOT_SRC")
esp_bytes=$(( esp_bytes + $(stat -c%s "$KERNEL_ELF") ))
if [ -n "$INITFS_IMG" ] && [ -f "$INITFS_IMG" ]; then
    esp_bytes=$(( esp_bytes + $(stat -c%s "$INITFS_IMG") ))
fi
if [ -n "$ROOTFS_IMG" ] && [ -f "$ROOTFS_IMG" ]; then
    esp_bytes=$(( esp_bytes + $(stat -c%s "$ROOTFS_IMG") ))
fi

# 最低 64MB、かつ内容量に +32MB 余裕を持たせる。
esp_mb=$(( (esp_bytes / 1048576) + 32 ))
if [ "$esp_mb" -lt 64 ]; then
    esp_mb=64
fi

dd if=/dev/zero of="$EFI_IMG" bs=1M count="$esp_mb" status=none
mkdosfs -F 32 -n MOCHIOS "$EFI_IMG" >/dev/null

mmd -i "$EFI_IMG" ::/EFI ::/EFI/BOOT ::/System
mcopy -i "$EFI_IMG" "$BOOT_SRC" ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$EFI_IMG" "$KERNEL_ELF" ::/System/kernel.elf

if [ -n "$INITFS_IMG" ] && [ -f "$INITFS_IMG" ]; then
    mcopy -i "$EFI_IMG" "$INITFS_IMG" ::/System/initfs.img
else
    echo "Warning: initfs.ext2 not found; ISO will not include initfs.img" >&2
fi

if [ -n "$ROOTFS_IMG" ] && [ -f "$ROOTFS_IMG" ]; then
    mcopy -i "$EFI_IMG" "$ROOTFS_IMG" ::/System/rootfs.ext2
else
    echo "Warning: rootfs.ext2 not found; ISO will not include rootfs.ext2" >&2
fi

cp "$EFI_IMG" "$ISO_ROOT/efiboot.img"

xorriso -as mkisofs \
    -iso-level 3 \
    -full-iso9660-filenames \
    -volid "MOCHIOS" \
    -eltorito-alt-boot \
    -e efiboot.img \
    -no-emul-boot \
    -output "$OUTPUT_ISO" \
    "$ISO_ROOT" >/dev/null

echo "Created UEFI-bootable ISO: $OUTPUT_ISO"
