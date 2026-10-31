# homelab

A media server on one Linux PC. It is reachable only over Tailscale, behind one login.

| | |
|---|---|
| Watch | Jellyfin, Seerr for requests, Bazarr for subtitles |
| Listen | Navidrome, with Spotify playlists and Liked Songs synced in |
| Fetch | Sonarr, Radarr, Prowlarr, qBittorrent, Lidarr, slskd, Soularr |
| Home page | Homepage, with live numbers from each app |

Caddy is the only entrance and Authelia checks the login. Every app listens on 127.0.0.1.

## Layout

```
compose.yml    every container
config/        Caddy, Authelia, Homepage and the fetch gate
rust/          the homelab binary: Spotify sync, downloads, disk guard, metrics
scripts/       setup and maintenance
systemd/       timers that run the binary
```

## Install

Needs Linux with Docker Compose 2.30+ and user id 1000, a `/dev/dri/renderD*` GPU, Tailscale with HTTPS certificates, and Rust.

```sh
cp .env.example .env     # fill it in; the comments say how
./scripts/setup.sh       # secrets, login, core apps, timers
```

## Open the lab

1. Open `https://HOST/` and sign in as `LAB_USER`.
2. Finish the Jellyfin setup at `/video/`, with `LAB_USER` as the admin.
3. Open `/music/` once, so that Navidrome makes your account.
4. Finish the Seerr setup at `https://HOST:5055/` with your Jellyfin account.
5. Run `./scripts/set-password.sh` to give every app one password.
6. Run `homelab homepage-secrets && docker compose up -d homepage`.

## Optional

| Part | How |
|---|---|
| Music fetchers | `./scripts/start-music-fetch.sh` |
| Spotify sync | Make an app at developer.spotify.com with the redirect `http://127.0.0.1:18080/callback`, put its `SPOTIFY_CLIENT_ID` and `SPOTIFY_CLIENT_SECRET` in `DATA/secrets/spotify.env`, run `homelab sync-spotify --auth`, then `start-music-fetch.sh` again |
| Video fetchers | `docker compose up -d sonarr radarr prowlarr qbittorrent`; sign in to qBittorrent with the password from its log, turn off its Host header check, then connect the others to it |
| Subtitles | After the video fetchers: `./scripts/setup-bazarr.sh`, then step 6 again |
| Jellyfin with the lab login | `sudo ufw allow from 172.30.77.0/24 to TS_IP port 443 proto tcp`, then `./scripts/configure-jellyfin-sso.sh` |
| Encrypted data | `sudo ./scripts/setup-master-password.sh`; after each boot, `sudo ./scripts/vault-unlock.sh` |
| Graphs | Prometheus that scrapes `127.0.0.1:9102`, and Grafana on `127.0.0.1:3000` with the dashboard `homelab-media` at `/grafana`; without them, delete the Graphs group in `config/homepage/services.yaml` |
| Download alerts | `NOTIFY_CMD` in `.env`, e.g. `notify-send` |
| First albums | `DATA/priority-albums.txt`, one `Artist - Album` per line |

After a Rust change: `cargo install --locked --path rust`, then `systemctl --user restart homelab-lab-metrics`.
