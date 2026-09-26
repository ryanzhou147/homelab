#!/usr/bin/env bash
# Moves the lab's data (logins, service keys, S3 storage, databases) into a LUKS2 vault opened
# only by your master password, so nothing can be read from the drive while the vault is locked.
# Media stays outside the vault. After a reboot the lab stays off until scripts/vault-unlock.sh.
# Usually run by setup-master-password.sh; alone: sudo ./scripts/encrypt-data.sh
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "Run with sudo."; exit 1; }
OWNER=${SUDO_USER:?Run through sudo from your own account.}
cd "$(dirname "$0")/.."
source .env

IMAGE=/var/lib/homelab-vault/data.img
NAME=homelab
MAPPER=/dev/mapper/$NAME
SIZE=${VAULT_SIZE:-30G}  # sparse: the file only takes the space the data uses
compose() { runuser -u "$OWNER" -- docker compose "$@"; }

[ ! -e "$IMAGE" ] || { echo "$IMAGE already exists."; exit 1; }
! mountpoint -q "$DATA" || { echo "$DATA is already a mount."; exit 1; }
[ ! -e "$DATA/media" ] || { echo "Move media out of $DATA first (MEDIA in .env)."; exit 1; }
command -v cryptsetup >/dev/null || apt-get install -y cryptsetup

pw=${VAULT_PASSWORD:-}
if [ -z "$pw" ]; then
  read -rsp "Master password: " pw; echo
  read -rsp "Again: " pw2; echo
  [ "$pw" = "$pw2" ] || { echo "The passwords differ."; exit 1; }
fi

staging=$(mktemp -d)
cleanup() {
  mountpoint -q "$staging" && umount "$staging"
  [ -e "$MAPPER" ] && ! mountpoint -q "$DATA" && cryptsetup close "$NAME"
  rmdir "$staging" 2>/dev/null || true
}
trap cleanup EXIT

# 1. Create the vault. printf without a newline matches what the unlock prompt reads.
install -d -m 700 "$(dirname "$IMAGE")"
truncate -s "$SIZE" "$IMAGE" && chmod 600 "$IMAGE"
printf '%s' "$pw" | cryptsetup luksFormat --type luks2 --batch-mode --key-file - "$IMAGE"
printf '%s' "$pw" | cryptsetup open --key-file - "$IMAGE" "$NAME"
mkfs.ext4 -q -L homelab-data "$MAPPER"

# 2. Copy the data in while the lab is stopped.
mount "$MAPPER" "$staging"
compose down
rsync -aHAX "$DATA"/ "$staging"/
umount "$staging"

# 3. Mount the vault over the old folder. The empty folder underneath is immutable, so while the
#    vault is locked nothing can write lab data unencrypted (the containers just fail to start).
mv "$DATA" "$DATA.plain-old"
mkdir -m 755 "$DATA" && chattr +i "$DATA"
mount "$MAPPER" "$DATA"

# 4. Start the lab from the vault, then delete the unencrypted copy.
compose up -d
for _ in $(seq 30); do
  compose ps authelia --format '{{.Status}}' | grep -q healthy && break
  sleep 2
done
compose ps authelia --format '{{.Status}}' | grep -q healthy ||
  { echo "The lab did not come up; old data kept in $DATA.plain-old."; exit 1; }
rm -rf "$DATA.plain-old"
# Deleting only unlinks files; TRIM tells the SSD to discard the freed blocks so the old plaintext
# can't be read back from the drive.
fstrim -v "$(findmnt -no TARGET -T "$(dirname "$DATA")")"
echo "Lab data is encrypted. After a reboot run: sudo ./scripts/vault-unlock.sh"
