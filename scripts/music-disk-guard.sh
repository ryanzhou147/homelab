#!/usr/bin/env bash
# Stops the music downloads when the media disk gets low, because slskd has no size limit of its
# own. A systemd user timer runs it every 10 minutes. Start them again with
#   docker compose start slskd soularr
set -euo pipefail
cd "$(dirname "$0")/.."
source .env
MIN_FREE_GB=20
NOTIFY="$HOME/.claude/harness/notify.py"

free_bytes=$(df --output=avail -B1 "$MEDIA" | tail -1 | tr -dc 0-9)
[ "$free_bytes" -lt $((MIN_FREE_GB * 1024 ** 3)) ] || exit 0
free_gb=$((free_bytes / 1024 ** 3))
[ -n "$(docker compose ps -q --status running slskd soularr)" ] || exit 0

docker compose stop soularr slskd
message="Music downloads stopped: only ${free_gb} GB free on the media disk."
echo "$message"
if [ -x "$NOTIFY" ]; then "$NOTIFY" "$message"; fi
