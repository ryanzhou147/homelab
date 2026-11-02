#!/usr/bin/env bash
# Writes .env from .env.example with this PC's values: its Tailscale name and address, your user
# name as the lab login, folders in your home, and the GPU if there is one. setup.sh runs it when
# there is no .env; edit .env after it to change a value.
#   ./scripts/write-env.sh
set -euo pipefail
cd "$(dirname "$0")/.."
[ ! -e .env ] || { echo ".env exists: edit it, or move it away to write a new one."; exit 1; }
command -v tailscale >/dev/null || { echo "Install Tailscale first: https://tailscale.com/download"; exit 1; }
user=${USER:-$(id -un)}
host=$(tailscale status --json 2>/dev/null | python3 -c 'import json, sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))' 2>/dev/null || true)
ip=$(tailscale ip -4 2>/dev/null | head -1 || true)
[ -n "$host" ] && [ -n "$ip" ] || { echo "Tailscale is not signed in: run 'sudo tailscale up'."; exit 1; }
render=$(ls /dev/dri/renderD* 2>/dev/null | head -1 || true)
gid=${render:+$(stat -c %g "$render")}
sed -e "s|^HOST=.*|HOST=$host|" -e "s|^TS_IP=.*|TS_IP=$ip|" \
	-e "s|^DATA=.*|DATA=$HOME/homelab-data|" -e "s|^MEDIA=.*|MEDIA=$HOME/homelab-media|" \
	-e "s|^LAB_USER=.*|LAB_USER=$user|" -e "s|^RENDER_DEVICE=.*|RENDER_DEVICE=$render|" \
	-e "s|^RENDER_GID=.*|RENDER_GID=$gid|" .env.example >.env
echo "Wrote .env: $host, login $user${render:+, GPU $render}."
