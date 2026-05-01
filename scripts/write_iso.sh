#!/usr/bin/env bash
set -euo pipefail

# write_iso.sh
# Create GPT (ESP + rootfs) on a target block device and install mochiOS files.
# Usage: sudo ./scripts/write_iso.sh /dev/sdX [ISO_PATH] [ROOTFS_IMG]
# If only /dev/sdX is given, defaults are used and missing artifacts are built automatically.

ISO_PATH=${2:-target/mochiOS.iso}
ROOTFS_IMG=${3:-target/mochiOS.img}
DEV=${1:-}

require_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Error: required command not found: $1" >&2
        exit 1
    fi
}

require_cmd xorriso
require_cmd parted
require_cmd mkfs.vfat
require_cmd mount
require_cmd umount
require_cmd dd
require_cmd sync
require_cmd partprobe || true

if [ -z "$DEV" ]; then
    echo "Usage: $0 /dev/sdX [ISO_PATH] [ROOTFS_IMG]" >&2
    exit 1
fi

if [ "$(id -u)" -ne 0 ]; then
    echo "This script must be run as root (sudo)." >&2
    exit 1
fi

if [ ! -b "$DEV" ]; then
    echo "Error: $DEV is not a block device." >&2
    exit 1
fi

# If artifacts missing, attempt to create them automatically
if [ ! -f "$ROOTFS_IMG" ]; then
    echo "[+] Rootfs image not found: $ROOTFS_IMG"
    if [ -x ./scripts/make_image.sh ]; then
        echo "[+] Running ./scripts/make_image.sh to build rootfs image..."
        ./scripts/make_image.sh || { echo "make_image.sh failed" >&2; exit 1; }
    else
        echo "Error: ./scripts/make_image.sh not found or not executable. Create $ROOTFS_IMG manually." >&2
        exit 1
    fi
fi

if [ ! -f "$ISO_PATH" ]; then
    echo "[+] ISO not found: $ISO_PATH"
    if [ -x ./scripts/create_iso.sh ]; then
        echo "[+] Running ./scripts/create_iso.sh to build ISO..."
        ./scripts/create_iso.sh || { echo "create_iso.sh failed" >&2; exit 1; }
    else
        echo "Error: ./scripts/create_iso.sh not found or not executable. Create $ISO_PATH manually." >&2
        exit 1
    fi
fi

TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

echo "[+] Extracting efiboot.img from ISO..."
xorriso -osirrox on -indev "$ISO_PATH" -extract /efiboot.img "$TMP_DIR/efiboot.img"

if [ ! -f "$TMP_DIR/efiboot.img" ]; then
    echo "Error: efiboot.img not found inside ISO" >&2
    exit 1
fi

MOUNT_EFI="$TMP_DIR/mnt_efiboot"
mkdir -p "$MOUNT_EFI"
mount -o loop "$TMP_DIR/efiboot.img" "$MOUNT_EFI"

# compute partition device names (handle nvme/mmcblk which need 'p' separator)
DEV_BASE=$(basename "$DEV")
if [[ "$DEV_BASE" =~ [0-9]$ ]]; then
    PART_SUFFIX="p"
else
    PART_SUFFIX=""
fi
PART1="${DEV}${PART_SUFFIX}1"
PART2="${DEV}${PART_SUFFIX}2"

cat <<EOF
Planned actions:
 - Wipe partition table on $DEV and create GPT with 2 partitions:
   1) ESP FAT32 512MiB -> $PART1
   2) rootfs ext2      -> $PART2 (rest of disk)
 - Copy EFI and System from efiboot.img into ESP
 - Write $ROOTFS_IMG to $PART2

WARNING: This will DESTROY all data on $DEV. Continue? Type YES to proceed:
EOF

read -r CONFIRM
if [ "$CONFIRM" != "YES" ]; then
    echo "Aborted by user." >&2
    umount "$MOUNT_EFI" || true
    exit 1
fi

# Try to unmount any mounted partitions of this device
for mp in $(lsblk -ln -o MOUNTPOINT "$DEV"* 2>/dev/null | awk 'NF'); do
    umount "$mp" || true
done || true

echo "[+] Creating GPT partition table on $DEV..."
parted -s "$DEV" mklabel gpt
parted -s "$DEV" mkpart ESP fat32 1MiB 513MiB
parted -s "$DEV" set 1 boot on
parted -s "$DEV" mkpart rootfs ext2 513MiB 100%

# Inform kernel
partprobe "$DEV" || true
sleep 1

# Wait for partitions to appear
for i in 1 2; do
    pdev="${DEV}${PART_SUFFIX}${i}"
    tries=0
    until [ -b "$pdev" ] || [ $tries -ge 20 ]; do
        sleep 0.5
        tries=$((tries+1))
    done
    if [ ! -b "$pdev" ]; then
        echo "Error: partition $pdev did not appear" >&2
        umount "$MOUNT_EFI" || true
        exit 1
    fi
done

echo "[+] Formatting ESP ($PART1) as FAT32..."
mkfs.vfat -F 32 -n MOCHIOS "$PART1"

MOUNT_ESP="$TMP_DIR/mnt_esp"
mkdir -p "$MOUNT_ESP"
mount "$PART1" "$MOUNT_ESP"

echo "[+] Copying EFI/ and System/ to ESP..."
# Ensure destination dirs exist
mkdir -p "$MOUNT_ESP/EFI"
mkdir -p "$MOUNT_ESP/System"

cp -r "$MOUNT_EFI/EFI" "$MOUNT_ESP/" || true
cp -r "$MOUNT_EFI/System" "$MOUNT_ESP/" || true
sync

umount "$MOUNT_ESP"
umount "$MOUNT_EFI"

# Write rootfs image (overwrite partition content)
if [ -f "$ROOTFS_IMG" ]; then
    echo "[+] Writing rootfs image $ROOTFS_IMG -> $PART2"
    # Ensure partition is not mounted
    if mountpoint -q "$PART2"; then
        echo "Error: $PART2 is mounted, aborting" >&2
        exit 1
    fi
    dd if="$ROOTFS_IMG" of="$PART2" bs=4M status=progress conv=fsync || true
    sync
    echo "[+] rootfs written"
else
    echo "Warning: rootfs image not found: $ROOTFS_IMG" >&2
    echo "You can create it with: ./scripts/make_image.sh" >&2
fi

echo "[+] All done. You can now boot the machine from this USB (disable Secure Boot if necessary)."
exit 0
