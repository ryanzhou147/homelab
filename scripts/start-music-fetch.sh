#!/usr/bin/env bash
# Starts the music fetchers and makes them ready to use, in one step:
#   ./scripts/start-music-fetch.sh
# The lab entrance (Caddy) and login (Authelia) restart to load the /lidarr and /slskd routes,
# so the lab is offline for a few seconds and you sign in again.
set -euo pipefail
cd "$(dirname "$0")/.."
source .env

./scripts/setup-music-fetch.sh
docker compose up -d lidarr slskd soularr
docker compose restart authelia caddy

mkdir -p ~/.config/systemd/user
ln -sf "$PWD"/systemd/homelab-music-guard.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now homelab-music-guard.timer

DATA="$DATA" HOST="$HOST" ./scripts/configure-music-fetch.py
