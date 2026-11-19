#!/usr/bin/env bash
# Sets up Explo, a weekly "Discover Weekly" from ListenBrainz into Navidrome, and runs it once.
# It links Navidrome to ListenBrainz if it is not linked (asks for the token at
# https://listenbrainz.org/settings/), and Explo uses that link's token. Safe to run again: it keeps
# the settings file that exists.
#   ./scripts/setup-explo.sh
set -euo pipefail
cd "$(dirname "$0")/.."
source .env
umask 077
mkdir -p "$DATA/explo/config"
(umask 022 && mkdir -p "$MEDIA/music/explo")

# Navidrome's socket, which only Caddy mounts, trusts the user name as it trusts Caddy: no password.
navidrome() {
	docker run -i --rm --user 0 -v "$DATA/sockets:/sockets" curlimages/curl:8.11.1 -sS \
		--unix-socket /sockets/navidrome.sock -H "Remote-User: $LAB_USER" "$@"
}
LINK=http://localhost/music/api/listenbrainz/link
if [[ $(navidrome "$LINK" </dev/null) != *'"status":true'* ]]; then
	read -rsp "ListenBrainz user token (https://listenbrainz.org/settings/): " lb; echo
	[ -n "$lb" ] || { echo "No token given."; exit 1; }
	# The token goes through the environment and stdin, never a command line, which others can see.
	reply=$(LB=$lb python3 -c 'import json, os; print(json.dumps({"token": os.environ["LB"]}))' |
		navidrome -X PUT -H 'Content-Type: application/json' --data-binary @- "$LINK")
	[[ $reply == *'"status":true'* ]] || { echo "Navidrome did not take the token: $reply"; exit 1; }
fi

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
	[ -n "$token" ] || { echo "Navidrome has no ListenBrainz link; run this again."; exit 1; }
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

./scripts/install-homelab.sh
mkdir -p ~/.config/systemd/user
ln -sf "$PWD"/systemd/homelab-discover.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now homelab-discover.timer
# The settings exist now, so setup does not ask again; the Monday timer tries again instead.
~/.cargo/bin/homelab discover || echo "The first weekly playlist failed; the Monday timer tries again."
