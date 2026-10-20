#!/usr/bin/env python3
"""Writes ${DATA}/status/downloads.json: how far the current spotdl run is, for the home page.

spotdl reports progress only in its log. The newer of the two fill logs holds the current run,
which starts at spotdl's "Processing query: <list file>" line; the list file gives the total.
The homelab-download-status timer runs this every minute.
"""

import json
import os
import subprocess
from pathlib import Path

DATA = Path(os.environ.get("DATA", Path.home() / "homelab-data"))
LOGS = [DATA / "exported-fill.log", DATA / "music-fill.log"]
OUT = DATA / "status" / "downloads.json"
DONE = 'Downloaded "'
SKIPPED = "(file already exists)"
# "no usable results" is only a retry warning; these lines end a song.
FAILED = ("LookupError: No results found", "AudioProviderError", "Error occurred while")


def status(log_text: str) -> dict:
    """Counts of the last spotdl run in a log."""
    _, start, run = log_text.rpartition("Processing query: ")
    if not start:
        return {"done": 0, "failed": 0, "total": 0, "left": 0, "percent": 0, "last": ""}
    list_file, _, body = run.partition("\n")
    try:
        total = len(json.loads(Path(list_file.strip()).read_text()))
    except (OSError, ValueError):
        total = 0
    lines = body.splitlines()
    done = [line for line in lines if DONE in line]
    failed = sum(any(word in line for word in FAILED) for line in lines)
    finished = len(done) + failed + sum(SKIPPED in line for line in lines)
    return {
        "done": len(done), "failed": failed, "total": total, "left": max(total - finished, 0),
        "percent": min(round(100 * finished / total), 100) if total else 0,
        "last": done[-1].split('"')[1] if done else "",
    }


def main() -> None:
    logs = [log for log in LOGS if log.exists()]
    newest = max(logs, key=lambda log: log.stat().st_mtime) if logs else None
    result = status(newest.read_text(errors="replace")) if newest else status("")
    running = subprocess.run(["pgrep", "-f", "bin/spotdl download"], capture_output=True, check=False).returncode == 0
    OUT.parent.mkdir(exist_ok=True)
    tmp = OUT.with_suffix(".tmp")
    tmp.write_text(json.dumps({**result, "running": running}))
    tmp.replace(OUT)


if __name__ == "__main__":
    main()
