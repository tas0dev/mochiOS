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

BOOTX64="$TEMP_DIR/BOOTX64.EFI"
cp "$SRC" "$BOOTX64"

# kernel.elf
PROFILE="debug"
case "$SRC" in
    */release/*) PROFILE="release" ;;
esac
FALLBACK_KERNEL="$ROOT_DIR/target/kernel/x86_64-unknown-none/$PROFILE/kernel"
FS_KERNEL="$ROOT_DIR/fs/System/kernel.elf"

# kernelビルド成果物（target/kernel）を優先して使用し、
# fs/System 側の古い/壊れたコピーを避ける。
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

# initfs.ext2 -> System/initfs.img
INITFS_IMG=$(find "$ROOT_DIR/target/x86_64-unknown-uefi" -name "initfs.ext2" -not -path "*/kernel/*" 2>/dev/null | xargs ls -t 2>/dev/null | head -1)
if [ -z "$INITFS_IMG" ] || [ ! -f "$INITFS_IMG" ]; then
    echo "Warning: initfs.ext2 not found" >&2
fi

# rootfs.ext2 -> System/rootfs.ext2
ROOTFS_IMG=$(find "$ROOT_DIR/target/x86_64-unknown-uefi" -name "rootfs.ext2" -not -path "*/kernel/*" 2>/dev/null | xargs ls -t 2>/dev/null | head -1)
if [ -z "$ROOTFS_IMG" ] || [ ! -f "$ROOTFS_IMG" ]; then
    echo "Warning: rootfs.ext2 not found" >&2
fi

# ── 実ESPイメージを構築（VVFATの代わり） ──────────────────────────────────
# ファイルの合計サイズを計算してFAT32イメージのサイズを決める
esp_bytes=0
for f in "$BOOTX64" "$KERNEL_ELF" "$INITFS_IMG" "$ROOTFS_IMG"; do
    [ -f "$f" ] && esp_bytes=$(( esp_bytes + $(stat -c%s "$f") ))
done
# 50MB のパディングを追加してMB単位に切り上げ
esp_mb=$(( (esp_bytes / 1048576) + 50 ))
echo "ESP image: ${esp_mb} MB (content: $((esp_bytes / 1048576)) MB)"

ESP_IMG="$TEMP_DIR/esp.img"
dd if=/dev/zero of="$ESP_IMG" bs=1M count="$esp_mb" status=none
mkdosfs -F 32 -n EFI "$ESP_IMG" > /dev/null

mmd -i "$ESP_IMG" ::/EFI ::/EFI/BOOT ::/System

mcopy -i "$ESP_IMG" "$BOOTX64" ::/EFI/BOOT/BOOTX64.EFI
echo "BOOTX64.EFI -> esp/EFI/BOOT/"

if [ -f "$KERNEL_ELF" ]; then
    mcopy -i "$ESP_IMG" "$KERNEL_ELF" ::/System/kernel.elf
    echo "kernel.elf  -> esp/System/"
fi

if [ -n "$INITFS_IMG" ] && [ -f "$INITFS_IMG" ]; then
    mcopy -i "$ESP_IMG" "$INITFS_IMG" ::/System/initfs.img
    echo "initfs.img  -> esp/System/ ($(( $(stat -c%s "$INITFS_IMG") / 1048576 )) MB)"
fi

if [ -n "$ROOTFS_IMG" ] && [ -f "$ROOTFS_IMG" ]; then
    mcopy -i "$ESP_IMG" "$ROOTFS_IMG" ::/System/rootfs.ext2
    echo "rootfs.ext2 -> esp/System/ ($(( $(stat -c%s "$ROOTFS_IMG") / 1048576 )) MB)"
fi

# KVM アクセラレーション（利用可能な場合）
KVM_ARGS=()
if [ -e /dev/kvm ] && [ -r /dev/kvm ]; then
    KVM_ARGS=(-enable-kvm -cpu host)
    echo "KVM acceleration enabled"
else
    echo "Warning: KVM not available, running without hardware acceleration" >&2
fi

exec qemu-system-x86_64 \
    "${KVM_ARGS[@]}" \
    -bios "$OVMF" \
    -drive format=raw,file="$ESP_IMG",media=disk \
    -drive id=disk0,file=target/mochiOS.img,format=raw,if=ide,index=1,media=disk \
    -usb \
    -device qemu-xhci,id=xhci \
    -device usb-kbd,bus=xhci.0 \
    -device usb-tablet,bus=xhci.0 \
    -netdev user,id=net0 \
    -device virtio-net-pci,netdev=net0 \
    -m 512M \
    -no-reboot \
    -d int,guest_errors \
    -D qemu.log \
    -serial stdio \
    -vga std
