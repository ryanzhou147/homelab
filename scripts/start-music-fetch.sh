#!/usr/bin/env bash
# Starts the music fetchers and makes them ready to use, in one step:
#   ./scripts/start-music-fetch.sh
# The lab entrance (Caddy) and login (Authelia) restart only when their config changed after they
# started. Then the lab is offline for a few seconds and you sign in again.
set -euo pipefail
cd "$(dirname "$0")/.."
source .env

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
systemctl --user daemon-reload
systemctl --user enable --now homelab-music-guard.timer

DATA="$DATA" HOST="$HOST" ./scripts/configure-music-fetch.py
