#!/usr/bin/env bash
# Sets up Jellyfin, Navidrome and Seerr with the lab login, so a new install needs no browser
# steps. setup.sh runs it every time: each step checks what is already done and does only the
# rest, so a run that failed part-way finishes on the next one. It asks for the lab password only
# when a step needs it (setup.sh passes the one just chosen in LAB_PASSWORD).
# The addresses below can be changed through the environment (for tests on spare ports).
set -Eeuo pipefail
cd "$(dirname "$0")/.."
NAVIDROME_ENV=${NAVIDROME_ENV:-} # kept from the caller: .env below sets DATA
source .env
NAVIDROME_ENV=${NAVIDROME_ENV:-$DATA/secrets/navidrome.env}
: "${LAB_USER:?Set LAB_USER in .env}"
JELLYFIN=${JELLYFIN:-http://127.0.0.1:8096}
SEERR=${SEERR:-http://127.0.0.1:5055}/api/v1
SEERR_JELLYFIN_HOST=${SEERR_JELLYFIN_HOST:-jellyfin} # Seerr reaches Jellyfin by this name
NAVIDROME_CONTAINER=${NAVIDROME_CONTAINER:-$(docker compose ps -q navidrome)}
RESTART_JELLYFIN=${RESTART_JELLYFIN:-docker compose restart jellyfin}
# Set but empty means "no key" (no colon in the expansion).
JELLYFIN_KEY=${JELLYFIN_KEY-$(sed -n 's/^HOMEPAGE_VAR_JELLYFIN_KEY=//p' "$DATA/secrets/homepage.env" 2>/dev/null || true)}
BASE=/video # Caddy serves Jellyfin at /video and keeps the prefix
WAIT_TRIES=60

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# The line only, never the command: a command line can hold the password.
trap 'echo "first-run.sh: stopped at line $LINENO; run it again to finish" >&2' ERR
json() { python3 -c "import json, sys; d = json.load(sys.stdin); print($1)"; }
# The password goes into request bodies through files, never on a command line.
body() { local expr=$1; shift; python3 -c "import json, os, sys; print(json.dumps($expr))" "$@" >"$tmp/body"; }
password() {
	if [ -z "${LAB_PASSWORD:-}" ]; then
		local again
		read -rsp "Lab password for $LAB_USER: " LAB_PASSWORD || { echo "first-run.sh needs the lab password" >&2; exit 1; }
		echo
		read -rsp "Again: " again || exit 1
		echo
		[ "$LAB_PASSWORD" = "$again" ] || { echo "The passwords differ" >&2; exit 1; }
	fi
	export LAB_PASSWORD
}
export LAB_USER HOST BASE

# Jellyfin. It answers at /video once its base URL is set, and at / before that.
AUTH='MediaBrowser Client="homelab", Device="setup", DeviceId="homelab-setup", Version="1"'
info= restarted=
for _ in $(seq $WAIT_TRIES); do
	for J in "$JELLYFIN$BASE" "$JELLYFIN"; do
		info=$(curl -sf "$J/System/Info/Public" || true)
		[[ $info == *'"Version"'* ]] && break 2
	done
	# A base URL saved before a restart that failed: Jellyfin sends / to it but serves it only after one.
	if [ -z "$restarted" ] && [[ $(curl -s -o /dev/null -w '%{redirect_url}' "$JELLYFIN/System/Info/Public") == *"$BASE/"* ]]; then
		$RESTART_JELLYFIN >/dev/null
		restarted=1
	fi
	sleep 3
done
[[ $info == *'"Version"'* ]] || { echo "Jellyfin did not start: docker compose logs jellyfin"; exit 1; }
if [ "$(json 'd.get("StartupWizardCompleted")' <<<"$info")" != True ]; then
	password
	post() { curl -sf -o /dev/null -X POST -H 'Content-Type: application/json' "$@"; }
	post -d '{"UICulture":"en-US","MetadataCountryCode":"US","PreferredMetadataLanguage":"en"}' "$J/Startup/Configuration"
	curl -sf -o /dev/null "$J/Startup/User"
	body '{"Name": os.environ["LAB_USER"], "Password": os.environ["LAB_PASSWORD"]}'
	post -d @"$tmp/body" "$J/Startup/User"
	post -d '{"EnableRemoteAccess":true,"EnableAutomaticPortMapping":false}' "$J/Startup/RemoteAccess"
	post "$J/Startup/Complete"
	echo "Jellyfin: admin $LAB_USER made"
fi
# The home page's API key once it exists and works (it can be from an older Jellyfin), else a
# sign-in with the password.
# The header goes through a file, so ps does not show the token.
printf 'Authorization: MediaBrowser Token="%s"\n' "$JELLYFIN_KEY" >"$tmp/ja"
if [ -z "$JELLYFIN_KEY" ] || ! curl -sf -o /dev/null -H @"$tmp/ja" "$J/System/Info"; then
	password
	body '{"Username": os.environ["LAB_USER"], "Pw": os.environ["LAB_PASSWORD"]}'
	token=$(curl -sf -X POST -H 'Content-Type: application/json' -H "Authorization: $AUTH" -d @"$tmp/body" \
		"$J/Users/AuthenticateByName" | json 'd["AccessToken"]')
	printf 'Authorization: %s, Token="%s"\n' "$AUTH" "$token" >"$tmp/ja"
fi
curl -sf -H @"$tmp/ja" "$J/System/Configuration/network" >"$tmp/net"
if [ "$(json 'd.get("BaseUrl")' <"$tmp/net")" != "$BASE" ]; then
	BASE=$BASE python3 -c 'import json, os, sys; print(json.dumps({**json.load(sys.stdin), "BaseUrl": os.environ["BASE"]}))' <"$tmp/net" >"$tmp/net.new"
	curl -sf -o /dev/null -X POST -H @"$tmp/ja" -H 'Content-Type: application/json' -d @"$tmp/net.new" "$J/System/Configuration/network"
	$RESTART_JELLYFIN >/dev/null
	J=$JELLYFIN$BASE
	for _ in $(seq $WAIT_TRIES); do curl -sf -o /dev/null "$J/System/Info/Public" && break; sleep 3; done
	curl -sf -o /dev/null "$J/System/Info/Public" || { echo "Jellyfin did not come back at $BASE"; exit 1; }
	echo "Jellyfin: served at $BASE"
fi
have=$(curl -sf -H @"$tmp/ja" "$J/Library/VirtualFolders" | json '" ".join(f["Name"] for f in d)')
for lib in "Movies movies /media/video/movies" "Shows tvshows /media/video/shows"; do
	set -- $lib
	[[ " $have " == *" $1 "* ]] && continue
	curl -sf -o /dev/null -X POST -H @"$tmp/ja" -H 'Content-Type: application/json' \
		-d "{\"LibraryOptions\":{\"PathInfos\":[{\"Path\":\"$3\"}]}}" \
		"$J/Library/VirtualFolders?name=$1&collectionType=$2&refreshLibrary=true"
	echo "Jellyfin: library $1"
done

# Navidrome: the admin account. Its CLI reads the password only from a terminal, so give it one.
nd_users() { docker exec "$NAVIDROME_CONTAINER" /app/navidrome user list --datafolder /data 2>/dev/null | cut -d, -f2; }
for _ in $(seq $WAIT_TRIES); do users=$(nd_users) && break; sleep 3; done
users=$(nd_users) || { echo "Navidrome did not answer: docker compose logs navidrome" >&2; exit 1; }
if grep -qxF "$LAB_USER" <<<"$users"; then
	[ -s "$NAVIDROME_ENV" ] || { echo "Navidrome has $LAB_USER but no $NAVIDROME_ENV: run ./scripts/set-password.sh" >&2; exit 1; }
else
	password
	# Written first: the Spotify sync and the home page sign in to Navidrome with this password.
	(umask 077 && printf 'NAVIDROME_PASSWORD=%s\n' "$LAB_PASSWORD" >"$NAVIDROME_ENV")
	for _ in 1 2 3; do
		(sleep 3; printf '%s\r' "$LAB_PASSWORD"; sleep 2; printf '%s\r' "$LAB_PASSWORD"; sleep 4) |
			script -qec "docker exec -it $NAVIDROME_CONTAINER /app/navidrome user create -u $LAB_USER --admin --datafolder /data" /dev/null >/dev/null || true
		users=$(nd_users) && grep -qxF "$LAB_USER" <<<"$users" && break
	done
	users=$(nd_users) && grep -qxF "$LAB_USER" <<<"$users" || { echo "Navidrome: could not make $LAB_USER; run this again"; exit 1; }
	echo "Navidrome: admin $LAB_USER made"
fi

# Seerr: the first Jellyfin sign-in makes the admin and saves Jellyfin; then the libraries.
for _ in $(seq $WAIT_TRIES); do curl -sf -o /dev/null "$SEERR/status" && break; sleep 3; done
public=$(curl -sf "$SEERR/settings/public")
if [ "$(json 'd.get("initialized")' <<<"$public")" != True ]; then
	password
	if [ "$(json 'd.get("mediaServerType")' <<<"$public")" = 2 ]; then
		# Jellyfin is already saved; Seerr refuses its address a second time.
		body '{"username": os.environ["LAB_USER"], "password": os.environ["LAB_PASSWORD"]}'
	else
		body '{"username": os.environ["LAB_USER"], "password": os.environ["LAB_PASSWORD"],
			"hostname": sys.argv[1], "port": 8096, "useSsl": False, "urlBase": os.environ["BASE"],
			"email": os.environ["LAB_USER"] + "@" + os.environ["HOST"], "serverType": 2}' "$SEERR_JELLYFIN_HOST"
	fi
	jar=(-b "$tmp/jar" -c "$tmp/jar")
	curl -sf -o /dev/null "${jar[@]}" -X POST -H 'Content-Type: application/json' -d @"$tmp/body" "$SEERR/auth/jellyfin"
	ids=$(curl -sf "${jar[@]}" -X POST "$SEERR/settings/jellyfin/library/sync" | json '" ".join(l["id"] for l in d)')
	[ -n "$ids" ] || { echo "Seerr found no Jellyfin libraries; run this again"; exit 1; }
	for id in $ids; do
		curl -sf -o /dev/null "${jar[@]}" -X PUT -H 'Content-Type: application/json' -d '{"enabled":true}' \
			"$SEERR/settings/jellyfin/library/$id"
	done
	curl -sf -o /dev/null "${jar[@]}" -X POST "$SEERR/settings/initialize"
	echo "Seerr: set up with Jellyfin"
fi
echo "First run: done"
