#!/usr/bin/env bash
# Starts the music fetchers and makes them ready to use, in one step:
#   ./scripts/start-music-fetch.sh
# The lab entrance (Caddy) and login (Authelia) restart only when their config changed after they
# started. Then the lab is offline for a few seconds and you sign in again.
set -euo pipefail
cd "$(dirname "$0")/.."
source .env
cargo install --quiet --locked --path rust  # the systemd units run ~/.cargo/bin/homelab

./scripts/setup-music-fetch.sh
docker compose up -d lidarr slskd soularr

restart_if_changed() {
	local service=$1 config=$2 started
	started=$(docker inspect -f '{{.State.StartedAt}}' "$(docker compose ps -q "$service")")
	if [ "$(date -r "$config" +%s)" -gt "$(date -d "$started" +%s)" ]; then
		echo "Restarting $service: $config changed after it started."
		docker compose restart "$service"
	fi
}
restart_if_changed authelia authelia/configuration.yml
restart_if_changed caddy Caddyfile

mkdir -p ~/.config/systemd/user
ln -sf "$PWD"/systemd/homelab-music-guard.{service,timer} ~/.config/systemd/user/
ln -sf "$PWD"/systemd/homelab-spotify-sync.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now homelab-music-guard.timer
# The Spotify sync needs --auth run once first; only enable the timer if credentials exist.
if [ -f "$DATA/secrets/spotify.env" ] && grep -q SPOTIFY_REFRESH_TOKEN "$DATA/secrets/spotify.env" 2>/dev/null; then
	systemctl --user enable --now homelab-spotify-sync.timer
else
	echo "Spotify sync: run 'homelab sync-spotify --auth' once, then re-run this script."
fi

DATA="$DATA" HOST="$HOST" ~/.cargo/bin/homelab configure-music-fetch
