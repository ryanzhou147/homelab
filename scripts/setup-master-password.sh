#!/usr/bin/env bash
# One master password, known only to you, for everything: the lab login, Jellyfin, Navidrome
# and the encrypted vault holding the lab's data.
# Deletes every stored copy of the password. Run once: sudo ./scripts/setup-master-password.sh
# If you forget the master password, the lab's data cannot be recovered.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "Run with sudo."; exit 1; }
OWNER=${SUDO_USER:?Run through sudo from your own account.}
cd "$(dirname "$0")/.."
source .env
: "${LAB_USER:?Set LAB_USER in .env}"
S="$DATA/secrets"
MIN_LENGTH=12

read -rsp "Current lab password: " current; echo
read -rsp "New master password: " pw; echo
read -rsp "Again: " pw2; echo
[ "$pw" = "$pw2" ] || { echo "The passwords differ."; exit 1; }
[ ${#pw} -ge $MIN_LENGTH ] || { echo "Use at least $MIN_LENGTH characters."; exit 1; }

# 1. Lab login, Jellyfin and Navidrome.
result=$(printf '%s\n%s\n%s\n' "$pw" "$pw" "$current" | runuser -u "$OWNER" -- ./scripts/set-password.sh "$LAB_USER")
echo "$result"
! grep -q failed <<<"$result" || { echo "Stopped: a login did not take the new password."; exit 1; }

# 2. Delete the stored copies of the passwords.
rm -f "$S/${LAB_USER}_password" "$S/jellyfin_admin_password" "$S/navidrome_app_password"

# 3. Move the lab's data into the vault locked by the same password (trims the SSD afterwards).
VAULT_PASSWORD=$pw ./scripts/encrypt-data.sh

code=$(curl -s -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' \
  -d "$(python3 -c 'import json,sys; print(json.dumps({"username": sys.argv[1], "password": sys.argv[2]}))' "$LAB_USER" "$pw")" \
  "https://$HOST/auth/api/firstfactor")
echo "Lab login with the master password: $([ "$code" = 200 ] && echo ok || echo "failed ($code)")"
echo "Done. After a reboot, unlock the lab with: sudo ./scripts/vault-unlock.sh"
