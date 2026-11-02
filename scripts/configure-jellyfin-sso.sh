#!/usr/bin/env bash
# Installs Jellyfin's SSO plugin, connects it to Authelia and adds a "Sign in with home lab login"
# button. setup.sh runs it after 'homelab homepage-secrets', whose Jellyfin key it uses. Safe to
# run again.
#   ./scripts/configure-jellyfin-sso.sh
set -euo pipefail
cd "$(dirname "$0")/.."
source .env
J=${JELLYFIN:-http://127.0.0.1:8096}/video
RESTART_JELLYFIN=${RESTART_JELLYFIN:-docker compose restart jellyfin}
S="$DATA/secrets"
REPO=https://raw.githubusercontent.com/9p4/jellyfin-plugin-sso/manifest-release/manifest.json
PLUGIN=505ce9d1d91642fa86ca673ef241d7df
KEY=$(sed -n 's/^HOMEPAGE_VAR_JELLYFIN_KEY=//p' "$S/homepage.env")
[ -n "$KEY" ] || { echo "No Jellyfin key: run 'homelab homepage-secrets' first."; exit 1; }
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# The key goes through a file, so ps does not show it.
printf 'Authorization: MediaBrowser Token="%s"\n' "$KEY" >"$tmp/auth"
jf() { curl -sf -H @"$tmp/auth" -H 'Content-Type: application/json' "$@"; }
pending() { jf "$J/System/Info" | python3 -c 'import json, sys; sys.exit(not json.load(sys.stdin)["HasPendingRestart"])'; }

if ! jf "$J/Plugins" | grep -q "\"$PLUGIN\""; then
	jf "$J/Repositories" | python3 -c 'import json, sys
repos = json.load(sys.stdin)
if all(r["Url"] != sys.argv[1] for r in repos):
    repos.append({"Name": "SSO", "Url": sys.argv[1], "Enabled": True})
print(json.dumps(repos))' "$REPO" >"$tmp/repos"
	jf -o /dev/null -X POST -d @"$tmp/repos" "$J/Repositories"
	jf -o /dev/null -X POST "$J/Packages/Installed/SSO%20Authentication?assemblyGuid=$PLUGIN&repositoryUrl=$REPO"
	# The download runs in the background; Jellyfin asks for a restart when it is done.
	for _ in $(seq 60); do pending && break; sleep 2; done
	pending || { echo "Jellyfin did not install the SSO plugin: docker compose logs jellyfin"; exit 1; }
	$RESTART_JELLYFIN >/dev/null
	for _ in $(seq 60); do jf -o /dev/null "$J/Plugins" && break; sleep 3; done
	jf "$J/Plugins" | grep -q "\"$PLUGIN\"" || { echo "The SSO plugin is not active after the restart."; exit 1; }
	echo "Jellyfin: SSO plugin installed"
fi

# Provider "authelia": admins group becomes Jellyfin admin; everyone else gets all libraries.
python3 - "$HOST" "$(cat "$S/jellyfin_oidc_secret")" >"$tmp/sso" <<'PY'
import json, sys
host, secret = sys.argv[1], sys.argv[2]
print(json.dumps({
    "OidEndpoint": f"https://{host}/auth", "OidClientId": "jellyfin", "OidSecret": secret,
    "Enabled": True, "EnableAuthorization": True, "EnableAllFolders": True,
    "Roles": [], "AdminRoles": ["admins"], "RoleClaim": "groups",
    "OidScopes": ["groups"], "SchemeOverride": "https", "DisablePushedAuthorization": True,
    "DefaultProvider": "", "NewPath": True,
}))
PY
jf -o /dev/null -X POST -d @"$tmp/sso" "$J/sso/OID/Add/authelia"

# Show "Home lab" instead of the container ID in Jellyfin's header and in the apps.
jf "$J/System/Configuration" |
	python3 -c 'import json,sys; c=json.load(sys.stdin); c["ServerName"]="Home lab"; json.dump(c,sys.stdout)' >"$tmp/config"
jf -o /dev/null -X POST -d @"$tmp/config" "$J/System/Configuration"

# Sign-in button on Jellyfin's login page.
jf "$J/System/Configuration/branding" | python3 -c '
import json, sys
c = json.load(sys.stdin)
c["LoginDisclaimer"] = "<form action=\"/video/sso/OID/start/authelia\"><button class=\"raised block emby-button button-submit\">Sign in with home lab login</button></form>"
json.dump(c, sys.stdout)' >"$tmp/brand"
jf -o /dev/null -X POST -d @"$tmp/brand" "$J/System/Configuration/branding"

# The plugin signs in through https://HOST/auth on the Tailscale address. A firewall that blocks
# the containers' range (compose.yml, networks) from it needs one rule.
reach() { docker compose exec -T jellyfin curl -so /dev/null -m 5 "https://$HOST/auth/api/health"; }
# ufw.service is "active" even with ufw off, so read its own setting.
if ! reach && grep -qs '^ENABLED=yes' /etc/ufw/ufw.conf; then
	echo "Jellyfin cannot reach https://$HOST/auth; adding a firewall rule (sudo):"
	sudo ufw allow from 172.30.77.0/24 to "$TS_IP" port 443 proto tcp
fi
reach || { echo "Jellyfin cannot reach https://$HOST/auth. If a firewall blocks 172.30.77.0/24 from $TS_IP port 443, allow it; else run this again later."; exit 1; }
echo "Jellyfin: sign-in button for the lab login"
