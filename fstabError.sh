#!/bin/bash

set -e

echo "[+] Backing up /etc/fstab..."

cp /etc/fstab /etc/fstab.backup

echo "[+] Introducing invalid mount entry..."

echo "/dev/nonexistent-device /mnt/test ext4 defaults 0 2" >> /etc/fstab

echo "[+] Breakage complete!"
echo "Run 'sudo mount -a' to observe the failure."



