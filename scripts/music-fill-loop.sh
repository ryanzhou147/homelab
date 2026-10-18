#!/usr/bin/env bash
# Runs until all playlist songs are in the library:
# 1. Downloads albums Soularr failed on via spotdl (using Spotify album URLs)
# 2. Rescans Navidrome
# 3. Updates Spotify playlists and downloads each missing track once with spotdl
# Run in background: setsid nohup scripts/music-fill-loop.sh >> ~/homelab-data/music-fill.log 2>&1 &
set -euo pipefail
cd "$(dirname "$0")/.."

DATA="${DATA:-$HOME/homelab-data}"
MUSIC="$HOME/homelab-media/music"
NAVIDROME_URL="${NAVIDROME_URL:-https://your-pc.your-tailnet.ts.net/music}"
NAVIDROME_USER="${NAVIDROME_USER:-admin}"
LIDARR="http://localhost:8686/lidarr"
LOG="$DATA/music-fill.log"
DONE_FILE="$DATA/spotdl-done.txt"
MISSING="$DATA/missing.spotdl"
SPOTDL_FAILS=0
MAX_SPOTDL_FAILS=3
TRIED="$DATA/spotdl-tried.txt"

set -a; source "$DATA/secrets/navidrome.env"; source "$DATA/secrets/spotify.env"; source "$DATA/secrets/lidarr.env"; set +a
NAVIDROME_PASSWORD="${NAVIDROME_PASSWORD:-}"
LIDARR_KEY="${LIDARR__AUTH__APIKEY:?Missing LIDARR__AUTH__APIKEY in lidarr.env}"

log() { echo "[$(date '+%H:%M:%S')] $*" | tee -a "$LOG"; }

navidrome_scan() {
    curl -sf "$NAVIDROME_URL/rest/startScan.view?u=$NAVIDROME_USER&p=$NAVIDROME_PASSWORD&v=1.16.1&c=fill-loop&f=json" >/dev/null || true
    # Wait for scan to complete (poll until scanning=false).
    for _ in $(seq 1 30); do
        sleep 5
        SCANNING=$(curl -sf "$NAVIDROME_URL/rest/getScanStatus.view?u=$NAVIDROME_USER&p=$NAVIDROME_PASSWORD&v=1.16.1&c=fill-loop&f=json" \
          | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('subsonic-response',{}).get('scanStatus',{}).get('scanning','false'))" 2>/dev/null || echo "false")
        [ "$SCANNING" = "true" ] || break
    done
}

navidrome_count() {
    curl -sf "$NAVIDROME_URL/rest/getScanStatus.view?u=$NAVIDROME_USER&p=$NAVIDROME_PASSWORD&v=1.16.1&c=fill-loop&f=json" \
      | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('subsonic-response',{}).get('scanStatus',{}).get('count',0))" 2>/dev/null || echo 0
}

spotify_sync() {
    DATA="$DATA" NAVIDROME_URL="$NAVIDROME_URL" NAVIDROME_USER="$NAVIDROME_USER" \
      NAVIDROME_PASSWORD="$NAVIDROME_PASSWORD" \
      python3 scripts/sync-spotify.py --missing-out "$MISSING" 2>&1 \
      | grep -E "in library|^   [0-9]|rate limit|missing tracks written" | tee -a "$LOG" || true
}

# Each track gets one try: spotdl finds no match for some, and retrying them every round wastes hours.
download_missing() {
    local urls
    urls=$(python3 -c 'import json,sys; print("\n".join(s["url"] for s in json.load(open(sys.argv[1]))))' "$MISSING" 2>/dev/null || true)
    [ -n "$urls" ] || return 0
    log "spotdl: downloading $(wc -l <<< "$urls") missing tracks"
    # spotdl exits 0 when only some tracks fail, so a non-zero exit means nothing was really tried.
    if (cd "$MUSIC" && spotdl download "$MISSING" --format mp3 --bitrate 320k --threads 4 \
      --output "{album-artist}/{album} ({year})/{track-number} - {title}.{output-ext}") >> "$LOG" 2>&1; then
        echo "$urls" >> "$TRIED"
        SPOTDL_FAILS=0
    else
        SPOTDL_FAILS=$((SPOTDL_FAILS + 1))
        log "spotdl exited with an error ($SPOTDL_FAILS in a row); these tracks stay untried"
    fi
}

download_album() {
    local artist="$1" album="$2" key="$artist|||$album"
    # Only skip if a previous spotdl run completed fully for this album.
    grep -qxF "$key" "$DONE_FILE" 2>/dev/null && return 0
    log "Downloading: $artist - $album"
    # spotdl skips tracks already on disk (--overwrite skip is default).
    if DATA="$DATA" python3 scripts/spotdl-album.py "$artist" "$album" >> "$LOG" 2>&1; then
        echo "$key" >> "$DONE_FILE"
    else
        log "Failed: $artist - $album"
    fi
}

# Albums that Soularr struggles with, ordered by playlist impact.
PRIORITY_ALBUMS=(
    "Kendrick Lamar|DAMN."
    "Don Toliver|HARDSTONE PSYCHO"
    "Don Toliver|Love Sick"
    "Don Toliver|Life of a DON"
    "Kanye West|The Life of Pablo"
    "Kanye West|My Beautiful Dark Twisted Fantasy"
    "Kanye West|808s & Heartbreak"
    "Kanye West|Late Registration"
    "Baby Keem|The Melodic Blue"
    "A\$AP Rocky|AT.LONG.LAST.A\$AP"
    "J. Cole|Born Sinner"
    "J. Cole|The Off-Season"
    "SZA|SOS"
    "Playboi Carti|MUSIC"
    "Playboi Carti|Whole Lotta Red"
    "Frank Ocean|Blonde"
    "Mac Miller|Swimming"
    "Lil Uzi Vert|Eternal Atake"
    "Lil Uzi Vert|Luv Is Rage 2"
    "Vince Staples|Dark Times"
    "Kid Cudi|INSANO"
    "Kid Cudi|Man on the Moon: The End of Day"
    "Gunna|One of Wun"
    "French Montana|Jungle Rules"
    "Lil Wayne|Tha Carter III"
    "Lil Wayne|Tha Carter IV"
    "Lil Wayne|Tha Carter V"
    "JAY-Z|Reasonable Doubt"
    "JAY-Z|Watch the Throne"
    "Nas|Illmatic"
    "Nas|Hip Hop Is Dead"
    "Eminem|The Eminem Show"
    "Eminem|Relapse"
    "2Pac|Me Against the World"
    "2Pac|Until the End of Time"
    "The Notorious B.I.G.|Life After Death"
    "OutKast|Aquemini"
    "MF DOOM|MM..FOOD"
    "Earl Sweatshirt|Doris"
    "Pusha T|My Name Is My Name"
    "Childish Gambino|Camp"
    "50 Cent|Get Rich or Die Tryin'"
    "Drake|Scorpion"
    "Swedish House Mafia|Paradise Again"
    "EKKSTACY|Negative"
    "Joji|Nectar"
    "Joji|BALLADS 1"
    "Internet Money|B4 the Storm"
    "Molchat Doma|Этажи"
    "Hozier|Hozier"
    "Tame Impala|Currents"
    "Daft Punk|Random Access Memories"
    "Gorillaz|Cracker Island"
)

touch "$DONE_FILE"
log "=== music-fill-loop started (round 1) ==="

ROUND=0
COUNT=0
while true; do
    ROUND=$((ROUND + 1))
    log "--- round $ROUND ---"
    FREE_GB=$(df --output=avail -BG "$MUSIC" | tail -1 | tr -dc 0-9)
    log "Disk free: ${FREE_GB}GB"
    if [ "${FREE_GB:-0}" -lt 5 ]; then
        log "Disk below 5GB, stopping"
        break
    fi
    if [ "${FREE_GB:-0}" -ge 15 ]; then
        # Priority downloads via spotdl.
        for entry in "${PRIORITY_ALBUMS[@]}"; do
            IFS='|' read -r artist album <<< "$entry"
            download_album "$artist" "$album"
        done

        # Also pull remaining wanted albums from Lidarr.
        WANTED=$(curl -sf "$LIDARR/api/v1/wanted/missing?pageSize=200&monitored=true" \
          -H "X-Api-Key: $LIDARR_KEY" 2>/dev/null \
          | python3 -c "
import json,sys
data=json.load(sys.stdin)
for r in data.get('records',[]):
    a=r.get('artist',{}).get('artistName','')
    t=r.get('title','')
    if a and t: print(f'{a}|{t}')
" 2>/dev/null || echo "")

        while IFS='|' read -r artist album; do
            [ -n "$artist" ] || continue
            download_album "$artist" "$album"
        done <<< "$WANTED"
    fi

    log "Scanning Navidrome..."
    navidrome_scan
    COUNT=$(navidrome_count)
    log "Navidrome: $COUNT tracks"

    log "Syncing playlists..."
    rm -f "$MISSING"  # a failed sync then leaves no stale list for spotdl to retry
    spotify_sync

    STILL_MISSING=$(curl -sf "$LIDARR/api/v1/wanted/missing?pageSize=1&monitored=true" \
      -H "X-Api-Key: $LIDARR_KEY" 2>/dev/null \
      | python3 -c "import json,sys; print(json.load(sys.stdin).get('totalRecords',0))" 2>/dev/null || echo "?")
    log "Lidarr still wants: $STILL_MISSING albums"

    UNTRIED=$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))))' "$MISSING" 2>/dev/null || echo "?")
    if [ "$UNTRIED" = "0" ]; then
        log "Every missing track was tried once. Loop done."
        break
    fi
    if [ "${FREE_GB:-0}" -ge 15 ]; then
        download_missing
    fi
    if [ "$SPOTDL_FAILS" -ge "$MAX_SPOTDL_FAILS" ]; then
        log "spotdl failed $SPOTDL_FAILS rounds in a row. Loop stopped."
        break
    fi

    log "Sleeping 5 minutes before next round..."
    sleep 300
done

log "=== music-fill-loop complete ==="
~/.claude/harness/notify.py "Music fill loop done: Navidrome has $COUNT tracks, playlists updated" 2>/dev/null || true
