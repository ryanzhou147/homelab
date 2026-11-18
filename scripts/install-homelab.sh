#!/usr/bin/env bash
# Builds the homelab binary and puts it at ~/.cargo/bin/homelab, where the systemd units run it.
# It builds in Docker on glibc 2.31, so the PC needs no Rust and any Linux of the last five years
# runs it. The first build takes minutes; later ones reuse the cache.
#   ./scripts/install-homelab.sh
set -euo pipefail
cd "$(dirname "$0")/.."
CACHE=~/.cache/homelab-build # safe to delete; the next build then takes minutes again
mkdir -p ~/.cargo/bin "$CACHE"
# The source keeps its path: the binary finds the repo's .env through the path it was built at.
# Cargo does not rebuild when only that path changes, so each path gets its own build folder.
# The new file is renamed over the old one: a running binary cannot be written in place.
TARGET=/cache/target${PWD//\//_}
docker run --rm --user "$(id -u):$(id -g)" -e CARGO_HOME=/cache -e CARGO_TARGET_DIR="$TARGET" \
	-v "$PWD/rust:$PWD/rust:ro" -v "$CACHE:/cache" -v ~/.cargo/bin:/out -w "$PWD/rust" rust:1.91.1-slim-bullseye \
	sh -c 'cargo build --quiet --release --locked && cp "$CARGO_TARGET_DIR/release/homelab" /out/homelab.new && mv /out/homelab.new /out/homelab'

# yt-dlp, for the songs Soulseek lacks. The lab keeps its own copy of the official release in
# $DATA/bin, which 'homelab' updates with -U before each playlist job: YouTube changes often.
DATA=$(sed -n 's/^DATA=//p' .env 2>/dev/null || true)
DATA=${DATA:-$HOME/homelab-data}
if [ ! -x "$DATA/bin/yt-dlp" ]; then
	mkdir -p "$DATA/bin"
	curl -fsSL -o "$DATA/bin/yt-dlp.new" https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp
	chmod +x "$DATA/bin/yt-dlp.new" && mv "$DATA/bin/yt-dlp.new" "$DATA/bin/yt-dlp"
fi
