#!/usr/bin/env bash
# First run on a new PC: folders, random secrets, the admin's lab login, the containers, and the
# homelab binary with its systemd timers. The first run writes .env (see write-env.sh). Safe to
# run again: it keeps every secret and login that exists.
#   ./scripts/setup.sh
set -euo pipefail
cd "$(dirname "$0")/.."
# With sudo, .env would get root's home and name. Add yourself to the docker group instead.
[ "$EUID" -ne 0 ] || { echo "Run this as your own user, not with sudo."; exit 1; }
[ -f .env ] || ./scripts/write-env.sh
source .env
for v in HOST TS_IP DATA MEDIA LAB_USER; do
	[ -n "${!v:-}" ] || { echo "Set $v in .env."; exit 1; }
done
[ -z "${RENDER_DEVICE:-}" ] || [ -e "$RENDER_DEVICE" ] || { echo "No $RENDER_DEVICE: fix RENDER_DEVICE in .env (ls /dev/dri)."; exit 1; }
[ -z "${RENDER_DEVICE:-}" ] || [ -n "${RENDER_GID:-}" ] || { echo "Set RENDER_GID in .env (getent group render)."; exit 1; }
for tool in docker tailscale openssl curl python3 script; do
	command -v "$tool" >/dev/null || { echo "Install $tool first."; exit 1; }
done
# Caddy gets the ts.net certificate from tailscaled, which gives one only with HTTPS turned on.
if [ "${HOST_TLS:-tailscale}" = tailscale ] && ! tailscale status --json |
	python3 -c 'import json, sys; sys.exit(sys.argv[1] not in (json.load(sys.stdin).get("CertDomains") or []))' "$HOST"; then
	echo "Turn on HTTPS Certificates at https://login.tailscale.com/admin/dns, then run this again."
	exit 1
fi
AUTHELIA="docker run --rm authelia/authelia:4.39.28 authelia"
S="$DATA/secrets"

# Media folders stay readable by the apps; everything after this is private.
mkdir -p "$MEDIA"/{music,video/movies,video/shows,downloads/qbittorrent}
# Homepage does not start without these files, and its config folder is read-only to it.
for f in docker kubernetes proxmox; do [ -e "config/homepage/$f.yaml" ] || echo '# Not used.' >"config/homepage/$f.yaml"; done
[ -e config/homepage/bookmarks.yaml ] || echo '[]' >config/homepage/bookmarks.yaml
umask 077
# Not navidrome: it runs as root without capabilities, so Docker must make its folder.
mkdir -p "$DATA"/{caddy,authelia,jellyfin/config,jellyfin/cache,seerr,status,secrets,prometheus,grafana}

new_secret() { [ -s "$S/$1" ] || openssl rand -hex 32 >"$S/$1"; }
for name in authelia_jwt authelia_session authelia_storage authelia_oidc_hmac jellyfin_oidc_secret grafana_admin; do
	new_secret "$name"
done
[ -s "$S/authelia_oidc_key.pem" ] || openssl genrsa -out "$S/authelia_oidc_key.pem" 2048 2>/dev/null
if [ ! -s "$S/jellyfin_oidc_digest" ]; then
	digest=$($AUTHELIA crypto hash generate pbkdf2 --variant sha512 --password "$(cat "$S/jellyfin_oidc_secret")" |
		sed -n 's/^Digest: //p')
	[ -n "$digest" ] || { echo "Could not hash the Jellyfin client secret."; exit 1; }
	echo "$digest" >"$S/jellyfin_oidc_digest"
fi
# Compose needs the file; 'homelab homepage-secrets' fills it once the apps run.
touch "$S/homepage.env"

USERS="$DATA/authelia/users.yml"
if [ ! -f "$USERS" ]; then
	read -rsp "Password for the lab login $LAB_USER (12+ characters): " pw; echo
	[ ${#pw} -ge 12 ] || { echo "Use at least 12 characters."; exit 1; }
	hash=$($AUTHELIA crypto hash generate argon2 --password "$pw" | sed -n 's/^Digest: //p')
	[ -n "$hash" ] || { echo "Could not hash the password."; exit 1; }
	cat >"$USERS" <<EOF
# Home lab accounts. Add a friend: generate a hash with
#   docker run --rm authelia/authelia:4.39.28 authelia crypto hash generate argon2
users:
  $LAB_USER:
    displayname: $LAB_USER
    email: $LAB_USER@$HOST
    password: '$hash'
    groups: [admins]
EOF
fi

docker compose up -d caddy authelia homepage navidrome jellyfin seerr
# The top bar graphs: updated when they run, skipped when another program holds port 9090 or 3000.
if [ -n "$(docker compose --profile graphs ps -q prometheus grafana)" ] ||
	! ss -Hltn '( sport = :9090 or sport = :3000 )' | grep -q .; then
	docker compose --profile graphs up -d prometheus grafana
else
	echo "Ports 9090 or 3000 are taken: graphs not started (see README, Other settings)."
fi

./scripts/install-homelab.sh
mkdir -p ~/.config/systemd/user
ln -sf "$PWD"/systemd/*.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now homelab-download-status.timer homelab-lab-metrics.service

# Accounts in Jellyfin, Navidrome and Seerr; first-run.sh skips what is done, so a failed first
# install finishes here on the next run. The home page's keys hold the Jellyfin key the steps below use.
LAB_PASSWORD="${pw:-}" ./scripts/first-run.sh
~/.cargo/bin/homelab homepage-secrets
docker compose up -d homepage

# Runs a step until it works, for apps that need a minute on their first start; shows the last error.
retry() {
	local out try
	for try in $(seq 20); do
		out=$("$@" 2>&1) && { [ -z "$out" ] || echo "$out"; return; }
		# qBittorrent bans the address after 5 refused logins.
		[[ $out != *"refused the login"* ]] || break
		[ "$try" -gt 1 ] || echo "Waiting for the apps to start: ${1##*/} ${2:-}" >&2
		sleep 6
	done
	echo "$out" >&2
	return 1
}
# Downloads and subtitles: nothing downloads until you add Prowlarr indexers or ask for music. A
# step that fails does not stop the rest; the next run of this script finishes it.
later=()
./scripts/configure-jellyfin-sso.sh || later+=("Jellyfin sign-in")
docker compose up -d sonarr radarr prowlarr qbittorrent
retry ~/.cargo/bin/homelab configure-video-fetch || later+=("video downloads")
# The Soulseek login can take a minute; only that check runs again.
./scripts/start-music-fetch.sh || retry ~/.cargo/bin/homelab configure-music-fetch || later+=("music downloads")
./scripts/setup-bazarr.sh || later+=(subtitles)
# Again, with the keys of the apps above, for the home page's cards.
~/.cargo/bin/homelab homepage-secrets
docker compose up -d homepage
[ ${#later[@]} -eq 0 ] || echo "Not ready yet: ${later[*]}. Run ./scripts/setup.sh again to finish."
echo "Running. Open https://$HOST and sign in as $LAB_USER."
