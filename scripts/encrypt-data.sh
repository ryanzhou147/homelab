#!/usr/bin/env bash
# Moves the lab's data (logins, secrets, S3 storage, service databases) into an encrypted LUKS2
# vault that the TPM unlocks at boot, so the data can't be read from an SSD taken out of the PC.
# Media stays outside the vault. Run once from your own account: sudo ./scripts/encrypt-data.sh
# Prints a recovery key: save it, it is the only way in if the TPM can't unlock the vault.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "Run with sudo."; exit 1; }
OWNER=${SUDO_USER:?Run through sudo from your own account.}
cd "$(dirname "$0")/.."
source .env

VAULT_DIR=/var/lib/homelab-vault
IMAGE=$VAULT_DIR/data.img
NAME=homelab
MAPPER=/dev/mapper/$NAME
SIZE=${VAULT_SIZE:-30G}  # sparse: the file only takes the space the data uses
TPM_PCRS=7               # Secure Boot policy; firmware and kernel updates don't change it
compose() { runuser -u "$OWNER" -- docker compose "$@"; }

[ ! -e "$IMAGE" ] || { echo "$IMAGE already exists."; exit 1; }
! mountpoint -q "$DATA" || { echo "$DATA is already a mount."; exit 1; }
[ ! -e "$DATA/media" ] || { echo "Move media out of $DATA first (MEDIA in .env)."; exit 1; }
command -v cryptsetup >/dev/null || apt-get install -y cryptsetup

key=$(mktemp -p /run)
staging=$(mktemp -d)
loop=""
cleanup() {
  mountpoint -q "$staging" && umount "$staging"
  [ -e "$MAPPER" ] && cryptsetup close "$NAME"
  [ -n "$loop" ] && losetup -d "$loop"
  shred -u "$key" 2>/dev/null || true
  rmdir "$staging" 2>/dev/null || true
}
trap cleanup EXIT

# 1. Create the vault with a temporary key, add the recovery key and the TPM, then drop the
#    temporary key.
head -c 64 /dev/urandom > "$key"
install -d -m 700 "$VAULT_DIR"
truncate -s "$SIZE" "$IMAGE" && chmod 600 "$IMAGE"
cryptsetup luksFormat --type luks2 --batch-mode --key-file "$key" "$IMAGE"
loop=$(losetup --find --show "$IMAGE")
echo
echo "==== RECOVERY KEY: save it in your password manager now. It opens the vault if the TPM"
echo "==== ever can't (BIOS reset, new motherboard). Nobody, including me, can recover it."
systemd-cryptenroll --unlock-key-file="$key" --recovery-key "$loop"
read -rp "Saved the recovery key? Type yes: " saved
[ "$saved" = yes ] || { echo "Stopped. Nothing was changed."; rm -f "$IMAGE"; exit 1; }
systemd-cryptenroll --unlock-key-file="$key" --tpm2-device=auto --tpm2-pcrs=$TPM_PCRS "$loop"

# 2. Copy the data in while the lab is stopped.
cryptsetup open --key-file "$key" "$loop" "$NAME"
mkfs.ext4 -q -L homelab-data "$MAPPER"
mount "$MAPPER" "$staging"
compose down
rsync -aHAX "$DATA"/ "$staging"/
umount "$staging"
cryptsetup close "$NAME"
systemd-cryptenroll --unlock-key-file="$key" --wipe-slot=0 "$loop"
losetup -d "$loop"; loop=""

# 3. Unlock at boot with the TPM and mount over the old folder. The empty folder underneath is
#    made immutable, so if the vault ever isn't mounted, nothing can write lab data unencrypted.
mv "$DATA" "$DATA.plain-old"
mkdir -m 755 "$DATA" && chattr +i "$DATA"
mount_unit=$(systemd-escape -p --suffix=mount "$DATA")
grep -q "^$NAME " /etc/crypttab 2>/dev/null ||
  echo "$NAME $IMAGE - tpm2-device=auto,nofail" >> /etc/crypttab
grep -q "^$MAPPER " /etc/fstab ||
  echo "$MAPPER $DATA ext4 defaults,noatime,nofail,x-systemd.requires=systemd-cryptsetup@$NAME.service 0 2" >> /etc/fstab
mkdir -p /etc/systemd/system/docker.service.d
printf '[Unit]\nWants=%s\nAfter=%s\n' "$mount_unit" "$mount_unit" \
  > /etc/systemd/system/docker.service.d/homelab-vault.conf
systemctl daemon-reload
systemctl start "systemd-cryptsetup@$NAME.service"   # unlocks with the TPM: the real boot path
systemctl start "$mount_unit"
mountpoint -q "$DATA" || { echo "The vault did not mount; old data is in $DATA.plain-old."; exit 1; }

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
echo "Done. Lab data is encrypted and unlocks with the TPM at boot."
echo "Passwords and keys that sat unencrypted before should now be changed: run"
echo "./scripts/set-password.sh for your password, and ask for the S3 key to be rotated."
