#!/usr/bin/env python3
"""Configures Lidarr for the music fetchers and checks that each piece is up.

1. Waits for Lidarr, and checks that it uses the /lidarr base and trusts Caddy's login.
2. Adds /data/music (the Navidrome library) as Lidarr's root folder, where a new artist wants no
   albums until you pick them.
3. Makes Lidarr refuse imports that would leave less than MIN_FREE_GB on the disk.
4. Checks that slskd is logged in to Soulseek and that Soularr reaches both apps.

    ./scripts/configure-music-fetch.py     (start-music-fetch.sh runs it)

Safe to run again: it changes only what differs.
"""

import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

DATA = Path(os.environ.get("DATA", Path.home() / "homelab-data"))
LIDARR = "http://127.0.0.1:8686/lidarr/api/v1"
SLSKD = "http://127.0.0.1:5030/slskd/api/v0"
ROOT_FOLDER = "/data/music"
MIN_FREE_GB = 20
START_TIMEOUT_S = 180
# A new artist adds no wanted albums by itself, so one click cannot queue a whole discography.
MONITOR_ONLY_PICKED = {"defaultMonitorOption": "none", "defaultNewItemMonitorOption": "none"}


def api_key() -> str:
    line = (DATA / "secrets/lidarr.env").read_text().strip()
    return line.split("=", 1)[1]


def call(url: str, key: str = "", method: str = "GET", body: object = None) -> object:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if key:
        req.add_header("X-Api-Key", key)
    with urllib.request.urlopen(req, timeout=15) as resp:
        return json.loads(resp.read() or b"null")


def wait_for_lidarr(key: str) -> dict:
    deadline = time.monotonic() + START_TIMEOUT_S
    while True:
        try:
            return call(f"{LIDARR}/system/status", key)
        except (urllib.error.URLError, ConnectionError) as err:
            if time.monotonic() > deadline:
                sys.exit(f"Lidarr did not answer within {START_TIMEOUT_S} s: {err}")
            time.sleep(3)


def ensure_root_folder(key: str) -> None:
    folders = call(f"{LIDARR}/rootfolder", key)
    existing = next((f for f in folders if f["path"].rstrip("/") == ROOT_FOLDER), None)
    if existing:
        if any(existing[k] != v for k, v in MONITOR_ONLY_PICKED.items()):
            call(f"{LIDARR}/rootfolder/{existing['id']}", key, "PUT",
                 {**existing, **MONITOR_ONLY_PICKED})
            print("Root folder now wants only the albums you pick.")
        return
    quality = call(f"{LIDARR}/qualityprofile", key)
    metadata = call(f"{LIDARR}/metadataprofile", key)
    if not quality or not metadata:
        sys.exit("Lidarr has no quality or metadata profile: add one in its settings, then rerun.")
    call(f"{LIDARR}/rootfolder", key, "POST", {
        "name": "Music",
        "path": ROOT_FOLDER,
        "defaultQualityProfileId": quality[0]["id"],
        "defaultMetadataProfileId": metadata[0]["id"],
        **MONITOR_ONLY_PICKED,
        "defaultTags": [],
    })
    print(f"Added root folder {ROOT_FOLDER}.")


def ensure_free_space_limit(key: str) -> None:
    config = call(f"{LIDARR}/config/mediamanagement", key)
    if config["minimumFreeSpaceWhenImporting"] == MIN_FREE_GB * 1024:
        return
    updated = {**config, "minimumFreeSpaceWhenImporting": MIN_FREE_GB * 1024}
    call(f"{LIDARR}/config/mediamanagement/{config['id']}", key, "PUT", updated)
    print(f"Lidarr now refuses imports below {MIN_FREE_GB} GB free.")


def check_slskd() -> bool:
    try:
        server = call(f"{SLSKD}/server")
    except urllib.error.URLError as err:
        print(f"FAIL slskd: {err}")
        return False
    ok = bool(server.get("isLoggedIn"))
    print(f"{'ok  ' if ok else 'FAIL'} slskd: Soulseek state {server.get('state')}")
    return ok


def check_soularr() -> bool:
    def docker(*args: str) -> str:
        # Tracebacks go to the container's stderr, so read both streams.
        return subprocess.run(["docker", *args], stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                              text=True, check=True, cwd=Path(__file__).parent.parent).stdout.strip()

    # Only the current run: a restarted container keeps the log of its earlier runs.
    container = docker("compose", "ps", "-q", "soularr")
    started = docker("inspect", "-f", "{{.State.StartedAt}}", container)
    logs = docker("logs", "--since", started, container)
    failed = [line for line in logs.splitlines() if "ERROR" in line or "Traceback" in line]
    for line in failed[-3:]:
        print(f"FAIL soularr: {line[-200:]}")
    if not failed:
        print("ok   soularr: no errors since it started")
    return not failed


def main() -> None:
    key = api_key()
    status = wait_for_lidarr(key)
    ok = status.get("urlBase") == "/lidarr" and status.get("authentication") == "external"
    print(f"{'ok  ' if ok else 'FAIL'} lidarr {status.get('version')}: base "
          f"{status.get('urlBase')!r}, login {status.get('authentication')!r}")
    ensure_root_folder(key)
    ensure_free_space_limit(key)
    ok = check_slskd() and ok
    ok = check_soularr() and ok
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
