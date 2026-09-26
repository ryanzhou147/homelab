#!/usr/bin/env bash
# Opens the lab's vault with your master password and starts the lab. Needed after every reboot.
# Run: sudo ./scripts/vault-unlock.sh   (also works over SSH from your phone or Mac via Tailscale)
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "Run with sudo."; exit 1; }
OWNER=${SUDO_USER:?Run through sudo from your own account.}
cd "$(dirname "$0")/.."
source .env

IMAGE=/var/lib/homelab-vault/data.img
NAME=homelab

if mountpoint -q "$DATA"; then
  echo "Already unlocked."
else
  [ -e "/dev/mapper/$NAME" ] || cryptsetup open "$IMAGE" "$NAME"
  mount "/dev/mapper/$NAME" "$DATA"
fi
runuser -u "$OWNER" -- docker compose up -d
echo "Lab unlocked and running."
