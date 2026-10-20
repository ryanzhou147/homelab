"""Tests for lab-metrics.py."""
import importlib.util
import os
from pathlib import Path

os.environ.setdefault("HOST", "lab.example")
_spec = importlib.util.spec_from_file_location(
    "lab_metrics", Path(__file__).parent.parent / "scripts" / "lab-metrics.py")
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)  # type: ignore[union-attr]


def test_render_writes_prometheus_text():
    text = _mod.render([("a_up", {"service": "x"}, 1), ("b", {}, 2.5)])
    assert text == 'a_up{service="x"} 1\nb 2.5\n'


def test_one_app_down_does_not_hide_the_others(monkeypatch):
    def fake_get(url, headers=None):
        if "sonarr" in url:
            raise OSError("down")
        return b'{"totalCount": 3, "totalRecords": 7}'

    for key in ("SONARR", "RADARR", "LIDARR"):
        monkeypatch.setenv(f"HOMEPAGE_VAR_{key}_KEY", "k")
    monkeypatch.setattr(_mod, "get", fake_get)
    monkeypatch.setattr(_mod, "HEALTH", {"sonarr": "http://sonarr/ping", "radarr": "http://radarr/ping"})
    samples = _mod.collect()
    apps = {labels["app"] for name, labels, _ in samples if name == "homelab_queue_items"}
    assert apps == {"radarr", "lidarr"}
    up = {labels["service"]: v for name, labels, v in samples if name == "homelab_service_up"}
    assert up == {"sonarr": 0, "radarr": 1}
