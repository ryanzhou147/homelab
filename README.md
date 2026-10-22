# homelab

A home media server on one Linux PC, reachable only over Tailscale, behind one login.

- **Watch:** Jellyfin for video, Seerr for show and movie requests, Bazarr for missing subtitles.
- **Listen:** Navidrome for music, with your Spotify playlists and Liked Songs synced into it.
- **Fetch:** Sonarr, Radarr, Prowlarr and qBittorrent for video. Lidarr, slskd and Soularr for music, with spotdl for the songs they cannot find.
- **Home page:** Homepage shows each service's health, live numbers and newest items.
- **Glue:** one Rust binary, `homelab`, runs the Spotify sync, the music downloads, the disk guard and a Prometheus exporter.

Caddy is the only entrance. It listens on the PC's Tailscale address, and Authelia checks the login. Each app listens on 127.0.0.1 only.

## Requirements

- Linux with Docker Compose 2.30 or later, and a user with the user id 1000 (`id -u`).
- A GPU with a `/dev/dri/renderD*` device, for video transcoding.
- Tailscale, with HTTPS certificates turned on for your tailnet.
- Rust (`cargo`), to build the `homelab` binary.

## Install

1. Copy the settings file: `cp .env.example .env`.
2. Fill in `.env`. Its comments give the commands that find each value.
3. Run `./scripts/setup.sh`. It asks for your lab password, then starts the core apps and the timers.

## Open the lab

1. Open `https://HOST/` and sign in as `LAB_USER`.
2. Open `/video/` and finish the Jellyfin setup. Use `LAB_USER` as the admin name.
3. Open `/music/` once, so that Navidrome makes your account.
4. Open `https://HOST:5055/` and finish the Seerr setup with your Jellyfin account.
5. Run `./scripts/set-password.sh` to give the lab login, Jellyfin and Navidrome one password.
6. Run `homelab homepage-secrets`, then `docker compose up -d homepage`. The home page cards then show live numbers.

## Optional parts

- **Music fetchers:** run `./scripts/start-music-fetch.sh`.
- **Spotify sync:** make an app at developer.spotify.com with the redirect URI `http://127.0.0.1:18080/callback`. Put its `SPOTIFY_CLIENT_ID` and `SPOTIFY_CLIENT_SECRET` in `DATA/secrets/spotify.env`. Run `homelab sync-spotify --auth` once, then run `./scripts/start-music-fetch.sh` again to turn on the daily timer.
- **Video fetchers:** run `docker compose up -d sonarr radarr prowlarr qbittorrent`. Sign in to qBittorrent with the temporary password from `docker compose logs qbittorrent`, and turn off its Host header check. Then connect Prowlarr, Sonarr and Radarr to qBittorrent in their own pages.
- **Subtitles:** once Sonarr or Radarr runs, run `./scripts/setup-bazarr.sh`, then `homelab homepage-secrets` and `docker compose up -d homepage`. Bazarr then finds a missing English subtitle for each show and movie. It uses only subtitle sites that need no account. For more matches, add an OpenSubtitles account in Bazarr's Providers page.
- **Sign in to Jellyfin with the lab login:** let the containers reach Caddy with `sudo ufw allow from 172.30.77.0/24 to TS_IP port 443 proto tcp`. Then run `./scripts/configure-jellyfin-sso.sh`.
- **Encrypted data:** run `sudo ./scripts/setup-master-password.sh`. After each reboot, unlock with `sudo ./scripts/vault-unlock.sh`.
- **Graphs:** the Graphs card shows panels of a Grafana dashboard with the uid `homelab-media`, served at `/grafana`. It needs Prometheus to scrape `127.0.0.1:9102` and Grafana on `127.0.0.1:3000`. Without them, delete the Graphs group from `homepage/services.yaml`.
- **Download alerts:** set `NOTIFY_CMD` in `.env`, for example to `notify-send`.
- **First albums to fetch:** list them in `DATA/priority-albums.txt`, one `Artist - Album` per line.

## The homelab binary

`homelab --help` lists the commands. The systemd units in `systemd/` run them, and `setup.sh` installs the units. After you change the Rust code, run `cargo install --locked --path rust`. Then restart the exporter with `systemctl --user restart homelab-lab-metrics`.
