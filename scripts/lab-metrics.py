#!/usr/bin/env python3
"""Prometheus exporter for the home lab apps, on 127.0.0.1:9102/metrics.

Each scrape asks the apps (through the PC's 127.0.0.1 ports, as the fetch gate requires) for
health, download speeds, queue and library sizes, and reads the spotdl progress file and the CPU
temperature. The API keys come from homepage.env (scripts/homepage-secrets.py). The Grafana
graphs on the home page are drawn from these numbers. Runs as homelab-lab-metrics.service.
"""

import json
import os
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from functools import partial
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

DATA = Path(os.environ.get("DATA", Path.home() / "homelab-data"))
HOST = os.environ["HOST"]  # the lab's name, from the repo's .env
LISTEN = ("127.0.0.1", 9102)
TIMEOUT = 3  # health checks, then *arr queue + wanted: 3 x 3 s stays under the 10 s scrape limit
HWMON = Path("/sys/class/hwmon")

HEALTH = {
    "jellyfin": "http://127.0.0.1:8096/video/health",
    "seerr": "http://127.0.0.1:5055/api/v1/status",
    "navidrome": f"https://{HOST}/music/rest/ping.view",
    "lidarr": "http://127.0.0.1:8686/lidarr/ping",
    "slskd": "http://127.0.0.1:5030/slskd/health",
    "qbittorrent": "http://127.0.0.1:8081/api/v2/app/version",
    "sonarr": "http://127.0.0.1:8989/sonarr/ping",
    "radarr": "http://127.0.0.1:7878/radarr/ping",
    "prowlarr": "http://127.0.0.1:9696/prowlarr/ping",
    "authelia": "http://127.0.0.1:9091/api/health",
    "homepage": "http://127.0.0.2:3010/api/healthcheck",
    "ai_gateway": "http://127.0.0.1:8100/health",
}
ARR = {  # app: (API root, key variable)
    "sonarr": ("http://127.0.0.1:8989/sonarr/api/v3", "HOMEPAGE_VAR_SONARR_KEY"),
    "radarr": ("http://127.0.0.1:7878/radarr/api/v3", "HOMEPAGE_VAR_RADARR_KEY"),
    "lidarr": ("http://127.0.0.1:8686/lidarr/api/v1", "HOMEPAGE_VAR_LIDARR_KEY"),
}


def get(url: str, headers: dict | None = None) -> bytes:
    # Homepage answers only to the lab's name.
    headers = {**({"Host": HOST} if "127.0.0.2" in url else {}), **(headers or {})}
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=TIMEOUT) as r:
        return r.read()


def up(url: str) -> int:
    try:
        get(url)
        return 1
    except Exception:  # noqa: BLE001  any failure means down
        return 0


def cpu_temperature() -> float | None:
    """Tctl of the Ryzen (k10temp), in degrees C."""
    for chip in HWMON.glob("hwmon*"):
        if (chip / "name").read_text().strip() == "k10temp":
            return int((chip / "temp1_input").read_text()) / 1000
    return None


def collect() -> list[tuple[str, dict, float]]:
    samples: list[tuple[str, dict, float]] = []
    with ThreadPoolExecutor(len(HEALTH)) as pool:
        for name, value in zip(HEALTH, pool.map(up, HEALTH.values())):
            samples.append(("homelab_service_up", {"service": name}, value))

    def safe(name, fn):
        try:
            fn()
        except Exception as err:  # noqa: BLE001  one app down must not hide the others
            print(f"lab-metrics: {name}: {err}", flush=True)

    def qbittorrent():
        info = json.loads(get("http://127.0.0.1:8081/api/v2/transfer/info"))
        samples.append(("homelab_qbittorrent_bytes_per_second", {"direction": "down"}, info["dl_info_speed"]))
        samples.append(("homelab_qbittorrent_bytes_per_second", {"direction": "up"}, info["up_info_speed"]))

    def arr(app, root, key):
        auth = {"X-Api-Key": os.environ[key]}
        queue = json.loads(get(f"{root}/queue/status", auth))["totalCount"]
        wanted = json.loads(get(f"{root}/wanted/missing?pageSize=1", auth))["totalRecords"]
        samples.append(("homelab_queue_items", {"app": app}, queue))
        samples.append(("homelab_wanted_items", {"app": app}, wanted))

    def library():
        auth = {"Authorization": f'MediaBrowser Token="{os.environ["HOMEPAGE_VAR_JELLYFIN_KEY"]}"'}
        counts = json.loads(get("http://127.0.0.1:8096/video/Items/Counts", auth))
        samples.append(("homelab_library_items", {"kind": "movies"}, counts["MovieCount"]))
        samples.append(("homelab_library_items", {"kind": "episodes"}, counts["EpisodeCount"]))
        token, salt = os.environ["HOMEPAGE_VAR_NAVIDROME_TOKEN"], os.environ["HOMEPAGE_VAR_NAVIDROME_SALT"]
        scan = json.loads(get(f"https://{HOST}/music/rest/getScanStatus?u=admin&t={token}&s={salt}"
                              "&v=1.16.1&c=lab-metrics&f=json"))
        samples.append(("homelab_library_items", {"kind": "songs"},
                        scan["subsonic-response"]["scanStatus"]["count"]))

    def spotdl():
        status = json.loads((DATA / "status" / "downloads.json").read_text())
        for state in ("done", "failed", "left"):
            samples.append(("homelab_spotdl_songs", {"state": state}, status[state]))
        samples.append(("homelab_spotdl_running", {}, int(status["running"])))

    def temperature():
        celsius = cpu_temperature()
        if celsius is not None:
            samples.append(("homelab_cpu_temperature_celsius", {}, celsius))

    jobs = {fn.__name__: fn for fn in (qbittorrent, library, spotdl, temperature)}
    jobs |= {app: partial(arr, app, *conf) for app, conf in ARR.items()}
    # In parallel, so one hung app cannot push the scrape past Prometheus's 10 s timeout.
    with ThreadPoolExecutor(len(jobs)) as pool:
        list(pool.map(safe, jobs, jobs.values()))
    return samples


def render(samples: list[tuple[str, dict, float]]) -> str:
    lines = []
    for name, labels, value in samples:
        label_text = ",".join(f'{k}="{v}"' for k, v in labels.items())
        lines.append(f"{name}{{{label_text}}} {value}" if label_text else f"{name} {value}")
    return "\n".join(lines) + "\n"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/metrics":
            self.send_error(404)
            return
        body = render(collect()).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; version=0.0.4")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    ThreadingHTTPServer(LISTEN, Handler).serve_forever()
