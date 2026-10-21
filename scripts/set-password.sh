#!/usr/bin/env bash
# Sets one password for a user's lab login (Authelia), Jellyfin and Navidrome, then checks each.
# Run it in a terminal: ./scripts/set-password.sh [username]   (default: LAB_USER in .env)
set -euo pipefail
cd "$(dirname "$0")/.."
source .env
: "${LAB_USER:?Set LAB_USER in .env}"
USER_NAME=${1:-$LAB_USER}
USERS="$DATA/authelia/users.yml"
J=http://127.0.0.1:8096/video
JAUTH='MediaBrowser Client="homelab-setup", Device="cli", DeviceId="homelab-setup", Version="1.0"'
MIN_LENGTH=12

read -rsp "New password for $USER_NAME: " pw; echo
read -rsp "Again: " pw2; echo
[ "$pw" = "$pw2" ] || { echo "The passwords differ."; exit 1; }
[ ${#pw} -ge $MIN_LENGTH ] || { echo "Use at least $MIN_LENGTH characters."; exit 1; }
# Jellyfin needs the admin's current password to make the change. Nothing stores it.
read -rsp "Current password of the admin ($LAB_USER): " admin_pw; echo

json() { python3 -c 'import json,sys; print(json.dumps(dict(zip(sys.argv[1::2], sys.argv[2::2]))))' "$@"; }

# 1. Lab login: replace the user's argon2 hash in Authelia's users file.
hash=$(docker compose exec -T authelia authelia crypto hash generate argon2 --password "$pw" | sed -n 's/^Digest: //p')
[ -n "$hash" ] || { echo "Could not hash the password."; exit 1; }
python3 - "$USERS" "$USER_NAME" "$hash" <<'PY'
import re, sys
path, user, digest = sys.argv[1:]
text = open(path).read()
block = re.search(rf"^  {re.escape(user)}:\n(?:    .*\n)*", text, re.M)
if not block:
    sys.exit(f"No lab login named {user}.")
new = re.sub(r"^(    password: ).*$", lambda m: m[1] + digest, block[0], count=1, flags=re.M)
open(path, "w").write(text[:block.start()] + new + text[block.end():])
PY
docker compose restart authelia >/dev/null

# 2. Jellyfin: the admin changes the password. Jellyfin asks for the current one when the admin
#    changes their own, and ignores it for anyone else.
admin_token=$(curl -sf -X POST "$J/Users/AuthenticateByName" -H 'Content-Type: application/json' \
  -H "Authorization: $JAUTH" -d "$(json Username "$LAB_USER" Pw "$admin_pw")" |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["AccessToken"])')
H="Authorization: $JAUTH, Token=\"$admin_token\""
user_id=$(curl -sf "$J/Users" -H "$H" | python3 -c '
import json, sys
print(next(u["Id"] for u in json.load(sys.stdin) if u["Name"] == sys.argv[1]))' "$USER_NAME")
curl -sf -X POST "$J/Users/$user_id/Password" -H "$H" -H 'Content-Type: application/json' \
  -d "$(json CurrentPw "$admin_pw" NewPw "$pw")" -o /dev/null

# 3. Navidrome: its CLI only reads passwords from a terminal, so give it one.
(sleep 3; printf '%s\r' "$pw"; sleep 2; printf '%s\r' "$pw"; sleep 4) |
  script -qec "docker compose exec navidrome /app/navidrome user edit -u $USER_NAME --set-password --datafolder /data" /dev/null >/dev/null

# The Spotify sync and the home page sign in to Navidrome as the admin with this password.
if [ "$USER_NAME" = "$LAB_USER" ]; then
	(umask 077; printf 'NAVIDROME_PASSWORD=%s\n' "$pw" >"$DATA/secrets/navidrome.env")
fi

# Check each login with the new password.
sleep 5
B="https://$HOST"
lab=$(curl -s -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' \
  -d "$(json username "$USER_NAME" password "$pw")" "$B/auth/api/firstfactor")
jf=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$J/Users/AuthenticateByName" -H 'Content-Type: application/json' \
  -H "Authorization: $JAUTH" -d "$(json Username "$USER_NAME" Pw "$pw")")
nd=$(curl -s -G "$B/music/rest/ping.view" --data-urlencode "u=$USER_NAME" --data-urlencode "p=$pw" \
  -d v=1.16.1 -d c=setup -d f=json | python3 -c 'import json,sys; print(json.load(sys.stdin)["subsonic-response"]["status"])')
echo "Lab login: $([ "$lab" = 200 ] && echo ok || echo "failed ($lab)")"
echo "Jellyfin:  $([ "$jf" = 200 ] && echo ok || echo "failed ($jf)")"
echo "Music:     $nd"
[ "$USER_NAME" != "$LAB_USER" ] || echo "Home page: run 'homelab homepage-secrets', then 'docker compose up -d homepage'."
