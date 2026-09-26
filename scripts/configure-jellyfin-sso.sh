#!/usr/bin/env bash
# Connects Jellyfin's SSO plugin to Authelia and adds a "Sign in with home lab login" button.
# Run once, after Tailscale HTTPS certificates are enabled and TLS_MODE=tailscale in .env.
set -euo pipefail
cd "$(dirname "$0")/.."
source .env
J=http://127.0.0.1:8096/video
S="$DATA/secrets"
AUTH='MediaBrowser Client="homelab-setup", Device="cli", DeviceId="homelab-setup", Version="1.0"'
read -rsp "Your password (Jellyfin admin admin): " admin_pw; echo
token=$(curl -sf -X POST "$J/Users/AuthenticateByName" -H 'Content-Type: application/json' \
  -H "Authorization: $AUTH" \
  -d "$(python3 -c 'import json,sys; print(json.dumps({"Username": "admin", "Pw": sys.argv[1]}))' "$admin_pw")" |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["AccessToken"])')
H="Authorization: $AUTH, Token=\"$token\""

# Provider "authelia": admins group becomes Jellyfin admin; everyone else gets all libraries.
python3 - "$HOST" "$(cat "$S/jellyfin_oidc_secret")" > /tmp/jf_sso.json <<'PY'
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
curl -sf -X POST "$J/sso/OID/Add/authelia" -H "$H" -H 'Content-Type: application/json' -d @/tmp/jf_sso.json
rm -f /tmp/jf_sso.json

# Show "Home lab" instead of the container ID in Jellyfin's header and in the apps.
curl -sf "$J/System/Configuration" -H "$H" |
  python3 -c 'import json,sys; c=json.load(sys.stdin); c["ServerName"]="Home lab"; json.dump(c,sys.stdout)' |
  curl -sf -X POST "$J/System/Configuration" -H "$H" -H 'Content-Type: application/json' -d @-

# Sign-in button on Jellyfin's login page.
curl -sf "$J/System/Configuration/branding" -H "$H" | python3 -c '
import json, sys
c = json.load(sys.stdin)
c["LoginDisclaimer"] = "<form action=\"/video/sso/OID/start/authelia\"><button class=\"raised block emby-button button-submit\">Sign in with home lab login</button></form>"
json.dump(c, sys.stdout)' > /tmp/jf_brand.json
curl -sf -X POST "$J/System/Configuration/branding" -H "$H" -H 'Content-Type: application/json' -d @/tmp/jf_brand.json
rm -f /tmp/jf_brand.json
echo "SSO configured. Open https://$HOST/video and use the sign-in button."
