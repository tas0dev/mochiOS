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

MOCHIOS_IMG_DEFAULT="$ROOT_DIR/target/mochiOS.img"

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
FS_KERNEL="$ROOT_DIR/fs/system/kernel.elf"
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
MOCHIOS_IMG="$MOCHIOS_IMG_DEFAULT"

mkdir -p "$(dirname "$OUTPUT_ISO")"

TEMP_DIR=$(mktemp -d)
# shellcheck disable=SC2064
trap "rm -rf $TEMP_DIR" EXIT

ESP_IMG="$TEMP_DIR/esp.img"
ISO_ROOT="$TEMP_DIR/isoroot"
EFI_DIR="$ISO_ROOT/EFI/BOOT"
SYS_DIR="$ISO_ROOT/system"

mkdir -p "$EFI_DIR"
mkdir -p "$SYS_DIR"
cp "$BOOT_SRC" "$EFI_DIR/BOOTX64.EFI"
esp_bytes=$(stat -c%s "$BOOT_SRC")
esp_mb=$(( (esp_bytes / 1048576) + 8 ))
if [ "$esp_mb" -lt 16 ]; then
    esp_mb=16
fi

dd if=/dev/zero of="$ESP_IMG" bs=1M count="$esp_mb" status=none
# Small ESP images are better as FAT16 to avoid firmware edge-cases.
if [ "$esp_mb" -lt 33 ]; then
    mkdosfs -F 16 -n MOCHIOS "$ESP_IMG" >/dev/null
else
    mkdosfs -F 32 -n MOCHIOS "$ESP_IMG" >/dev/null
fi

mmd -i "$ESP_IMG" ::/EFI ::/EFI/BOOT ::/system
mcopy -i "$ESP_IMG" "$BOOT_SRC" ::/EFI/BOOT/BOOTX64.EFI
cp "$KERNEL_ELF" "$SYS_DIR/kernel.elf"
if [ -n "$INITFS_IMG" ] && [ -f "$INITFS_IMG" ]; then
    cp "$INITFS_IMG" "$SYS_DIR/initfs.img"
else
    echo "Warning: initfs.ext2 not found; ISO will not include system/initfs.img" >&2
fi

if [ -n "$ROOTFS_IMG" ] && [ -f "$ROOTFS_IMG" ]; then
    cp "$ROOTFS_IMG" "$SYS_DIR/rootfs.ext2"
else
    echo "Warning: rootfs.ext2 not found; ISO will not include system/rootfs.ext2" >&2
fi

cp "$ESP_IMG" "$ISO_ROOT/esp.img"

if [ -f "$MOCHIOS_IMG" ]; then
    cp "$MOCHIOS_IMG" "$ISO_ROOT/mochiOS.img"
else
    echo "Warning: $MOCHIOS_IMG not found; ISO will not include mochiOS.img" >&2
    echo "Hint: run scripts/make_image.sh (or cargo build if it invokes it) to generate target/mochiOS.img" >&2
fi

xorriso -as mkisofs \
    -iso-level 3 \
    -full-iso9660-filenames \
    -volid "MOCHIOS" \
    -eltorito-alt-boot \
    -e esp.img \
    -no-emul-boot \
    -output "$OUTPUT_ISO" \
    "$ISO_ROOT" >/dev/null

echo "Created UEFI-bootable ISO: $OUTPUT_ISO"
