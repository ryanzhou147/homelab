#!/usr/bin/env bash
# Starts Bazarr and makes it ready to use, in one step: an English profile for every show and
# movie, subtitle sites that need no account, and links to Sonarr and Radarr. Safe to run again:
# it keeps the profiles you changed in Bazarr.
#   ./scripts/setup-bazarr.sh
set -euo pipefail
cd "$(dirname "$0")/.."
source .env
umask 077
mkdir -p "$DATA/bazarr/config" "$DATA/secrets"
KEY_FILE="$DATA/secrets/bazarr_apikey"
[ -s "$KEY_FILE" ] || openssl rand -hex 16 >"$KEY_FILE"
KEY=$(cat "$KEY_FILE")

arr_key() { sed -n 's:.*<ApiKey>\(.*\)</ApiKey>.*:\1:p' "$DATA/$1/config.xml" 2>/dev/null || true; }
SONARR_KEY=$(arr_key sonarr)
RADARR_KEY=$(arr_key radarr)
[ -n "$SONARR_KEY$RADARR_KEY" ] || { echo "Start Sonarr or Radarr first."; exit 1; }

# Bazarr fills in every setting this file leaves out. No login of its own: it listens on the
# gate's 127.0.0.1 only, and Caddy lets in only admins.
CONFIG="$DATA/bazarr/config/config.yaml"
# A config that Bazarr wrote itself listens on every address of the gate's network, past the gate.
if [ -f "$CONFIG" ] && ! grep -q '^  ip: 127.0.0.1$' "$CONFIG"; then
	echo "$CONFIG listens on more than 127.0.0.1. Stop Bazarr, delete the file, and run this again."
	exit 1
fi
if [ ! -f "$CONFIG" ]; then
	cat >"$CONFIG" <<EOF
general:
  ip: 127.0.0.1
  port: 6767
  base_url: /bazarr
  serie_default_enabled: true
  serie_default_profile: 1
  movie_default_enabled: true
  movie_default_profile: 1
  enabled_providers: [gestdown, podnapisi, yifysubtitles, supersubtitles]
  auto_update: false
analytics:
  enabled: false
auth:
  apikey: $KEY
sonarr:
  base_url: /sonarr
radarr:
  base_url: /radarr
EOF
fi

docker compose up -d bazarr
# The new Bazarr port makes Docker recreate the gate. The apps on the gate's network then still
# use the old one, with no network, so recreate them too.
gate=$(docker compose ps -q fetch-gate)
stale=()
for service in $(docker compose ps --services --status running); do
	mode=$(docker inspect -f '{{.HostConfig.NetworkMode}}' "$(docker compose ps -q "$service")")
	[[ $mode != container:* || ${mode#container:} == "$gate" ]] || stale+=("$service")
done
[ ${#stale[@]} -eq 0 ] || docker compose up -d --force-recreate --no-deps "${stale[@]}"
API=http://127.0.0.1:6767/bazarr/api
# Keys go in through files, so they do not show in the process list.
api() { curl -fsS -H @<(printf 'X-API-KEY: %s\n' "$KEY") "$@"; }
for _ in $(seq 60); do api "$API/system/status" >/dev/null 2>&1 && break; sleep 2; done
api "$API/system/status" >/dev/null || { echo "Bazarr did not start: docker compose logs bazarr"; exit 1; }

# Sent on each run, so a Sonarr or Radarr added later gets linked too.
link() {
	[ -z "$2" ] || api -X POST "$API/system/settings" -F "settings-general-use_$1=true" \
		-F "settings-$1-apikey=<"<(printf %s "$2") >/dev/null
}
link sonarr "$SONARR_KEY"
link radarr "$RADARR_KEY"

# hi False: normal or hearing-impaired subtitles are both good.
PROFILE='[{"profileId":1,"name":"English","cutoff":null,"mustContain":[],"mustNotContain":[],
"originalFormat":null,"items":[{"id":1,"language":"en","audio_exclude":"False",
"audio_only_include":"False","hi":"False","forced":"False"}]}]'
if [ "$(api "$API/system/languages/profiles")" = "[]" ]; then
	api -X POST "$API/system/settings" -F languages-enabled=en -F "languages-profiles=$PROFILE" >/dev/null
fi

# Shows and movies that Bazarr knew before the profile have none, so give them the English one.
# $1: Bazarr's list, $2: its id field, $3: the form name of that id, $4 and $5: the Sonarr or Radarr
# list and its API key.
assign() {
	local kind=$1 field=$2 form=$3 want list="" have=0 ids
	want=$(curl -fsS -H @<(printf 'X-API-KEY: %s\n' "$5") "$4" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')
	api -X POST "$API/system/tasks?taskid=update_$kind" >/dev/null
	# The sync runs in the background: wait until Bazarr lists all of them.
	for _ in $(seq 90); do
		list=$(api "$API/$kind" || true)
		have=$(python3 -c 'import json,sys; print(len(json.load(sys.stdin)["data"]))' <<<"$list" 2>/dev/null || echo 0)
		[ "$have" -ge "$want" ] && break
		sleep 2
	done
	[ "$have" -ge "$want" ] || echo "Bazarr lists $have of $want $kind. Check its Sonarr and Radarr settings."
	ids=$(python3 -c "import json,sys; print(' '.join(str(x['$field']) for x in json.load(sys.stdin)['data'] if x['profileId'] is None))" <<<"$list" 2>/dev/null || true)
	echo "$kind: $have in Bazarr, $(wc -w <<<"$ids") get the English profile now."
	[ -n "$ids" ] || return 0
	local args=()
	for id in $ids; do args+=(-F "$form=$id" -F profileid=1); done
	api -X POST "$API/$kind" "${args[@]}" >/dev/null
}
[ -z "$SONARR_KEY" ] || assign series sonarrSeriesId seriesid http://127.0.0.1:8989/sonarr/api/v3/series "$SONARR_KEY"
[ -z "$RADARR_KEY" ] || assign movies radarrId radarrid http://127.0.0.1:7878/radarr/api/v3/movie "$RADARR_KEY"
api -X POST "$API/system/tasks?taskid=wanted_search_missing_subtitles_series" >/dev/null
api -X POST "$API/system/tasks?taskid=wanted_search_missing_subtitles_movies" >/dev/null

# Jellyfin then sees each new subtitle file at once, not at its next library scan.
JELLYFIN_KEY=$(sed -n 's/^HOMEPAGE_VAR_JELLYFIN_KEY=//p' "$DATA/secrets/homepage.env" 2>/dev/null || true)
if [ -n "$JELLYFIN_KEY" ]; then
	JELLYFIN_KEY=$JELLYFIN_KEY python3 - <<'PY'
import json, os, urllib.request
base = "http://127.0.0.1:8096/video/Library/VirtualFolders"
auth = {"Authorization": f'MediaBrowser Token="{os.environ["JELLYFIN_KEY"]}"'}
with urllib.request.urlopen(urllib.request.Request(base, headers=auth)) as r:
    folders = json.load(r)
for f in folders:
    options = {**f["LibraryOptions"], "EnableRealtimeMonitor": True}
    body = json.dumps({"Id": f["ItemId"], "LibraryOptions": options}).encode()
    headers = {**auth, "Content-Type": "application/json"}
    urllib.request.urlopen(urllib.request.Request(base + "/LibraryOptions", body, headers))
    print(f"Jellyfin watches {f['Name']} for new files.")
PY
else
	echo "Jellyfin finds new subtitles at its next library scan; run 'homelab homepage-secrets' and this script again to make it see them at once."
fi
