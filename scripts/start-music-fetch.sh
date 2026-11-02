#!/usr/bin/env bash
# Starts the music fetchers and makes them ready to use, in one step:
#   ./scripts/start-music-fetch.sh
# The lab entrance (Caddy) and login (Authelia) restart only when their config changed after they
# started. Then the lab is offline for a few seconds and you sign in again.
set -euo pipefail
cd "$(dirname "$0")/.."
source .env
./scripts/install-homelab.sh

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
restart_if_changed authelia config/authelia/configuration.yml
restart_if_changed caddy config/caddy/Caddyfile

mkdir -p ~/.config/systemd/user
ln -sf "$PWD"/systemd/homelab-music-guard.{service,timer} ~/.config/systemd/user/
ln -sf "$PWD"/systemd/homelab-spotify-sync.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
# The Spotify sync does nothing until 'homelab sync-spotify --auth' has run once.
systemctl --user enable --now homelab-music-guard.timer homelab-spotify-sync.timer

DATA="$DATA" HOST="$HOST" ~/.cargo/bin/homelab configure-music-fetch
