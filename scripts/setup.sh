#!/usr/bin/env bash
# First run on a new PC: folders, random secrets, the admin's lab login, the containers, and the
# homelab binary with its systemd timers. Fill in .env first (copy .env.example). Safe to run
# again: it keeps every secret and login that exists.
#   ./scripts/setup.sh
set -euo pipefail
cd "$(dirname "$0")/.."
[ -f .env ] || { echo "Copy .env.example to .env and fill it in first."; exit 1; }
source .env
for v in HOST TS_IP DATA MEDIA LAB_USER; do
	[ -n "${!v:-}" ] || { echo "Set $v in .env."; exit 1; }
done
[ -z "${RENDER_DEVICE:-}" ] || [ -e "$RENDER_DEVICE" ] || { echo "No $RENDER_DEVICE: fix RENDER_DEVICE in .env (ls /dev/dri)."; exit 1; }
[ -z "${RENDER_DEVICE:-}" ] || [ -n "${RENDER_GID:-}" ] || { echo "Set RENDER_GID in .env (getent group render)."; exit 1; }
for tool in docker openssl cargo curl python3 script; do
	command -v "$tool" >/dev/null || { echo "Install $tool first."; exit 1; }
done
AUTHELIA="docker run --rm authelia/authelia:4.39.28 authelia"
S="$DATA/secrets"

# Media folders stay readable by the apps; everything after this is private.
mkdir -p "$MEDIA"/{music,video/movies,video/shows,downloads/qbittorrent}
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
	echo "Ports 9090 or 3000 are taken: graphs not started (see README, Graphs)."
fi

cargo install --quiet --locked --path rust
mkdir -p ~/.config/systemd/user
ln -sf "$PWD"/systemd/*.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now homelab-download-status.timer homelab-lab-metrics.service

# Accounts in Jellyfin, Navidrome and Seerr; first-run.sh skips what is done, so a failed first
# install finishes here on the next run. Then the home page's keys, while it has none.
LAB_PASSWORD="${pw:-}" ./scripts/first-run.sh
if [ ! -s "$S/homepage.env" ]; then
	~/.cargo/bin/homelab homepage-secrets
	docker compose up -d homepage
fi

echo "Running. Open https://$HOST and sign in as $LAB_USER."
