#!/usr/bin/env python3
"""Writes ${DATA}/secrets/homepage.env: the API keys that the home page widgets use.

The keys come from each app's own settings, and Lidarr's from its env file. Jellyfin gets its
own "Homepage" key, made with the key that Seerr already has. Navidrome gets a Subsonic token
and salt made from its password.

    ./scripts/homepage-secrets.py     (then: docker compose up -d homepage)

Safe to run again: it uses the Jellyfin key that exists, and writes the file again.
"""

import hashlib
import json
import os
import re
import secrets
import urllib.request
from pathlib import Path

DATA = Path(os.environ.get("DATA", Path.home() / "homelab-data"))
JELLYFIN = "http://127.0.0.1:8096/video"
APP_NAME = "Homepage"


def arr_key(app: str) -> str:
    match = re.search(r"<ApiKey>(\w+)</ApiKey>", (DATA / app / "config.xml").read_text())
    if not match:
        raise SystemExit(f"No ApiKey in {DATA / app / 'config.xml'}")
    return match.group(1)


def jellyfin_key(admin_key: str) -> str:
    def call(path: str, method: str = "GET"):
        auth = {"Authorization": f'MediaBrowser Token="{admin_key}"'}
        req = urllib.request.Request(f"{JELLYFIN}{path}", method=method, headers=auth)
        with urllib.request.urlopen(req, timeout=10) as resp:
            body = resp.read()
        return json.loads(body) if body else None

    def find():
        return next((k["AccessToken"] for k in call("/Auth/Keys")["Items"] if k["AppName"] == APP_NAME), None)

    if not find():
        call(f"/Auth/Keys?app={APP_NAME}", "POST")
    key = find()
    if not key:
        raise SystemExit("Jellyfin did not make the Homepage API key")
    return key


def env_value(path: Path, name: str) -> str:
    match = re.search(rf"^{name}=(.*)$", path.read_text(), re.MULTILINE)
    if not match:
        raise SystemExit(f"No {name} in {path}")
    return match.group(1).strip().strip("'\"")


def main() -> None:
    seerr = json.loads((DATA / "seerr" / "settings.json").read_text())
    salt = secrets.token_hex(8)
    password = env_value(DATA / "secrets" / "navidrome.env", "NAVIDROME_PASSWORD")
    values = {
        "SONARR_KEY": arr_key("sonarr"),
        "RADARR_KEY": arr_key("radarr"),
        "LIDARR_KEY": env_value(DATA / "secrets" / "lidarr.env", "LIDARR__AUTH__APIKEY"),
        "PROWLARR_KEY": arr_key("prowlarr"),
        "SEERR_KEY": seerr["main"]["apiKey"],
        "JELLYFIN_KEY": jellyfin_key(seerr["jellyfin"]["apiKey"]),
        "NAVIDROME_SALT": salt,
        "NAVIDROME_TOKEN": hashlib.md5((password + salt).encode()).hexdigest(),
    }
    out = DATA / "secrets" / "homepage.env"
    out.touch(mode=0o600)
    out.chmod(0o600)  # touch keeps the mode of a file that exists
    out.write_text("".join(f"HOMEPAGE_VAR_{k}={v}\n" for k, v in values.items()))
    print(f"Wrote {len(values)} values to {out}")


if __name__ == "__main__":
    main()
