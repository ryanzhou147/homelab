#!/usr/bin/env bash
# One master password, known only to you, for everything: the lab login, Jellyfin, Navidrome,
# rclone's encrypted S3 folder and settings, and the encrypted vault holding the lab's data.
# Deletes every stored copy of the password. Run once: sudo ./scripts/setup-master-password.sh
# If you forget the master password, the lab's data and encrypted S3 files cannot be recovered.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "Run with sudo."; exit 1; }
OWNER=${SUDO_USER:?Run through sudo from your own account.}
OWNER_HOME=$(getent passwd "$OWNER" | cut -d: -f6)
cd "$(dirname "$0")/.."
source .env
S="$DATA/secrets"
RCLONE="$OWNER_HOME/.local/bin/rclone"
MIN_LENGTH=12

read -rsp "Current lab password: " current; echo
read -rsp "New master password: " pw; echo
read -rsp "Again: " pw2; echo
[ "$pw" = "$pw2" ] || { echo "The passwords differ."; exit 1; }
[ ${#pw} -ge $MIN_LENGTH ] || { echo "Use at least $MIN_LENGTH characters."; exit 1; }

# 1. Lab login, Jellyfin and Navidrome.
result=$(printf '%s\n%s\n%s\n' "$pw" "$pw" "$current" | runuser -u "$OWNER" -- ./scripts/set-password.sh admin)
echo "$result"
! grep -q failed <<<"$result" || { echo "Stopped: a login did not take the new password."; exit 1; }

# 2. rclone: the encrypted folder's key comes from the master password, and the settings file
#    (which holds the S3 keys) is locked with it too. Every rclone command then asks for it.
export MASTER_PW=$pw
as_owner() { sudo -u "$OWNER" -H --preserve-env=MASTER_PW "$@"; }
obscured=$(printf '%s' "$pw" | as_owner "$RCLONE" obscure -)
as_owner "$RCLONE" config delete vault 2>/dev/null || true
as_owner "$RCLONE" config create vault crypt remote=lab:personal/vault filename_encryption=standard \
  directory_name_encryption=true password="$obscured" --no-obscure >/dev/null
as_owner "$RCLONE" config encryption set --password-command "printenv MASTER_PW"
probe=$(mktemp); echo "master password check" > "$probe"; chmod 644 "$probe"
rc() { as_owner "$RCLONE" --password-command "printenv MASTER_PW" "$@"; }
rc copyto "$probe" vault:.check && rc cat vault:.check | cmp -s - "$probe" && rc deletefile vault:.check
rm -f "$probe"
echo "rclone: ok"

# 3. Delete the stored copies of passwords and keys.
rm -f "$S/ryan_password" "$S/jellyfin_admin_password" "$S/navidrome_app_password" \
  "$OWNER_HOME/.config/rclone/vault-keys.txt"

# 4. Move the lab's data into the vault locked by the same password (trims the SSD afterwards).
VAULT_PASSWORD=$pw ./scripts/encrypt-data.sh
unset MASTER_PW

code=$(curl -s -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' \
  -d "$(python3 -c 'import json,sys; print(json.dumps({"username": "admin", "password": sys.argv[1]}))' "$pw")" \
  "https://$HOST/auth/api/firstfactor")
echo "Lab login with the master password: $([ "$code" = 200 ] && echo ok || echo "failed ($code)")"
echo "Done. After a reboot, unlock the lab with: sudo ./scripts/vault-unlock.sh"
