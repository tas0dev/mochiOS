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
    echo "Error: OVMF firmware not found."
    exit 1
fi

SRC="$1"
if [ -z "$SRC" ]; then
    echo "Usage: $0 <EFI_FILE|BOOT_DIR>"
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="$ROOT_DIR/target"

rm -f "$TARGET_DIR/esp.img"

mkdir -p "$TARGET_DIR"
ESP_IMG="$TARGET_DIR/esp.img"

BOOTX64_SRC="$SRC"

PROFILE="debug"
[[ "$SRC" == */release/* ]] && PROFILE="release"

FALLBACK_KERNEL="$ROOT_DIR/target/kernel/x86_64-unknown-none/$PROFILE/kernel"
FS_KERNEL="$ROOT_DIR/fs/system/kernel.elf"
KERNEL_ELF="${FALLBACK_KERNEL}"
[ ! -f "$KERNEL_ELF" ] && KERNEL_ELF="$FS_KERNEL"

INITFS_IMG=$(find "$ROOT_DIR/target/x86_64-unknown-uefi" -name "initfs.ext2" 2>/dev/null | xargs ls -t 2>/dev/null | head -1 || true)
ROOTFS_IMG=$(find "$ROOT_DIR/target/x86_64-unknown-uefi" -name "rootfs.ext2" 2>/dev/null | xargs ls -t 2>/dev/null | head -1 || true)

esp_bytes=0
for f in "$BOOTX64_SRC" "$KERNEL_ELF" "$INITFS_IMG" "$ROOTFS_IMG"; do
    [ -f "$f" ] && esp_bytes=$(( esp_bytes + $(stat -c%s "$f") ))
done
esp_mb=$(( (esp_bytes / 1048576) + 50 ))

rm -f "$ESP_IMG"
dd if=/dev/zero of="$ESP_IMG" bs=1M count="$esp_mb" status=none
mkdosfs -F 32 -n EFI "$ESP_IMG" > /dev/null

mmd -i "$ESP_IMG" ::/EFI ::/EFI/BOOT ::/system
mcopy -i "$ESP_IMG" "$BOOTX64_SRC" ::/EFI/BOOT/BOOTX64.EFI

[ -f "$KERNEL_ELF" ] && mcopy -i "$ESP_IMG" "$KERNEL_ELF" ::/system/kernel.elf
[ -f "$INITFS_IMG" ] && mcopy -i "$ESP_IMG" "$INITFS_IMG" ::/system/initfs.img
[ -f "$ROOTFS_IMG" ] && mcopy -i "$ESP_IMG" "$ROOTFS_IMG" ::/system/rootfs.ext2

KVM_ARGS=()
if [ -e /dev/kvm ] && [ -r /dev/kvm ]; then
    KVM_ARGS=(-enable-kvm -cpu host,migratable=no,+invtsc)
fi

exec qemu-system-x86_64 \
    "${KVM_ARGS[@]}" \
    -bios "$OVMF" \
    -drive format=raw,file="$ESP_IMG",index=0,media=disk \
    -drive id=disk0,file="$TARGET_DIR/mochiOS.img",format=raw,if=ide,index=1,media=disk \
    -usb \
    -device qemu-xhci,id=xhci \
    -device usb-kbd,bus=xhci.0 \
    -device usb-tablet,bus=xhci.0 \
    -netdev user,id=net0 \
    -device virtio-net-pci,netdev=net0 \
    -m 512M \
    -no-reboot \
    -serial stdio \
    -vga std
