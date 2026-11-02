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
