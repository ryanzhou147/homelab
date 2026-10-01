#!/usr/bin/env bash
# One-time setup for the music fetchers (Lidarr, slskd, Soularr): folders, a Lidarr API key, a new
# Soulseek account and Soularr's config, all under $DATA (inside the vault). Existing files are kept.
#   ./scripts/setup-music-fetch.sh && docker compose up -d lidarr slskd soularr
set -euo pipefail
cd "$(dirname "$0")/.."
source .env

# Media folders stay readable by the other apps; everything after this is private.
mkdir -p "$MEDIA"/downloads/{slskd,incomplete} "$MEDIA/music"
umask 077
mkdir -p "$DATA"/{lidarr,slskd,soularr,secrets}

if [ ! -f "$DATA/secrets/lidarr.env" ]; then
	echo "LIDARR__AUTH__APIKEY=$(openssl rand -hex 16)" >"$DATA/secrets/lidarr.env"
fi

# Soulseek makes the account on first login. The password is random because the Soulseek
# protocol is not encrypted: never reuse the master password here.
if [ ! -f "$DATA/secrets/slskd.env" ]; then
	cat >"$DATA/secrets/slskd.env" <<EOF
SLSKD_SLSK_USERNAME=lab$(openssl rand -hex 4)
SLSKD_SLSK_PASSWORD=$(openssl rand -hex 16)
EOF
fi

if [ ! -f "$DATA/soularr/config.ini" ]; then
	source "$DATA/secrets/lidarr.env"
	# Based on Soularr v1.2.2's config.ini. MP3 320 comes first because the disk is nearly full.
	cat >"$DATA/soularr/config.ini" <<EOF
[Lidarr]
api_key = $LIDARR__AUTH__APIKEY
host_url = http://127.0.0.1:8686/lidarr
download_dir = /data/downloads/slskd
disable_sync = False

[Slskd]
api_key = unused-slskd-has-no-login
host_url = http://127.0.0.1:5030
url_base = /slskd
download_dir = /downloads
delete_searches = False
stalled_timeout = 3600
remote_queue_timeout = 300

[Release Settings]
use_selected_lidarr_release = False
use_most_common_tracknum = True
allow_multi_disc = True
accepted_countries = Europe,Japan,United Kingdom,United States,[Worldwide],Australia,Canada
skip_region_check = False
accepted_formats = CD,Digital Media,Vinyl

[Search Settings]
search_timeout = 5000
maximum_peer_queue = 50
minimum_peer_upload_speed = 0
minimum_filename_match_ratio = 0.8
minimum_search_interval = 5
allowed_filetypes = mp3 320,flac 16/44.1,flac,mp3
ignored_users =
album_prepend_artist = False
search_type = incrementing_page
number_of_albums_to_grab = 10
title_blacklist =
search_blacklist =
search_source = missing
failed_import_denylist = True

[Download Settings]
download_filtering = True
use_extension_whitelist = False
extensions_whitelist = lrc,nfo,txt

[Logging]
level = INFO
format = [%(levelname)s|%(module)s|L%(lineno)d] %(asctime)s: %(message)s
datefmt = %Y-%m-%dT%H:%M:%S%z
log_to_file = True
log_file = soularr.log
max_bytes = 1048576
backup_count = 3
EOF
fi
echo "Music fetch setup done. Start it: docker compose up -d lidarr slskd soularr"
