#!/usr/bin/env bash
# Sets up Explo, a weekly "Discover Weekly" from ListenBrainz into Navidrome, and runs it once.
# Link Navidrome to ListenBrainz first (Navidrome: Personal > ListenBrainz); Explo uses that link's
# token. Safe to run again: it keeps the settings file that exists.
#   ./scripts/setup-explo.sh
set -euo pipefail
cd "$(dirname "$0")/.."
source .env
umask 077
mkdir -p "$DATA/explo/config"
(umask 022 && mkdir -p "$MEDIA/music/explo")

SETTINGS="$DATA/explo/.env"
if [ ! -s "$SETTINGS" ]; then
	tmp=$(mktemp -d)
	trap 'rm -rf "$tmp"' EXIT
	# With the -wal file: a link made a moment ago may be only there.
	cp "$DATA"/navidrome/navidrome.db* "$tmp"/
	token=$(python3 -c 'import sqlite3, sys
row = sqlite3.connect(sys.argv[1]).execute(
    "select p.value from user_props p join user u on u.id = p.user_id"
    " where u.user_name = ? and p.key = ?", (sys.argv[2], "ListenBrainzSessionKey")).fetchone()
print(row[0] if row else "")' "$tmp/navidrome.db" "$LAB_USER")
	[ -n "$token" ] || { echo "Link Navidrome to ListenBrainz first: Personal > ListenBrainz."; exit 1; }
	user=$(curl -fsS -H @<(printf 'Authorization: Token %s\n' "$token") \
		https://api.listenbrainz.org/1/validate-token |
		python3 -c 'import json, sys; d = json.load(sys.stdin); print(d["user_name"] if d.get("valid") else "")')
	[ -n "$user" ] || { echo "ListenBrainz does not accept Navidrome's token; link it again."; exit 1; }
	password=$(sed -n 's/^NAVIDROME_PASSWORD=//p' "$DATA/secrets/navidrome.env")
	[ -n "$password" ] || { echo "No NAVIDROME_PASSWORD in $DATA/secrets/navidrome.env: run set-password.sh."; exit 1; }
	case $password in *"'"*) echo "Explo cannot read a password with ' in it: pick another with set-password.sh."; exit 1 ;; esac
	# Single quotes keep the values literal for Explo's .env reader.
	cat >"$SETTINGS" <<EOF
DISCOVERY_SERVICE=listenbrainz
LISTENBRAINZ_USER='$user'
LISTENBRAINZ_USER_TOKEN='$token'
EXPLO_SYSTEM=subsonic
SYSTEM_URL=https://$HOST/music
SYSTEM_USERNAME='$LAB_USER'
SYSTEM_PASSWORD='$password'
DOWNLOAD_SERVICES=youtube
USE_SUBDIRECTORY=true
EOF
fi

cargo install --quiet --locked --path rust # the timer runs ~/.cargo/bin/homelab discover
mkdir -p ~/.config/systemd/user
ln -sf "$PWD"/systemd/homelab-discover.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now homelab-discover.timer
~/.cargo/bin/homelab discover
