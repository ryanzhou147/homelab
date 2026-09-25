#!/usr/bin/env bash
# When the TPM can't unlock the lab's vault at boot (BIOS update or reset, Secure Boot turned on,
# new motherboard), the lab stays stopped. This asks for the recovery key, seals the vault to the
# TPM again and starts the lab. Run: sudo ./scripts/vault-recover.sh
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "Run with sudo."; exit 1; }
OWNER=${SUDO_USER:?Run through sudo from your own account.}
cd "$(dirname "$0")/.."
source .env

IMAGE=/var/lib/homelab-vault/data.img
NAME=homelab
TPM_PCRS=7

loop=$(losetup --find --show "$IMAGE")
trap 'losetup -d "$loop"' EXIT
echo "Enter the recovery key when asked."
systemd-cryptenroll --wipe-slot=tpm2 --tpm2-device=auto --tpm2-pcrs=$TPM_PCRS "$loop"
losetup -d "$loop"; trap - EXIT

systemctl restart "systemd-cryptsetup@$NAME.service"
systemctl start "$(systemd-escape -p --suffix=mount "$DATA")"
runuser -u "$OWNER" -- docker compose up -d
echo "Vault unlocked and sealed to the TPM again."
