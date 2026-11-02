<div align="center">

# homelab

Your own Netflix and Spotify, running at home.

<a href="https://jellyfin.org"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/jellyfin.svg" height="40" alt="Jellyfin" title="Jellyfin"></a>
<a href="https://github.com/seerr-team/seerr"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/seerr.svg" height="40" alt="Seerr" title="Seerr"></a>
<a href="https://www.bazarr.media"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/bazarr.svg" height="40" alt="Bazarr" title="Bazarr"></a>
<a href="https://www.navidrome.org"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/navidrome.svg" height="40" alt="Navidrome" title="Navidrome"></a>
<a href="https://sonarr.tv"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/sonarr.svg" height="40" alt="Sonarr" title="Sonarr"></a>
<a href="https://radarr.video"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/radarr.svg" height="40" alt="Radarr" title="Radarr"></a>
<a href="https://prowlarr.com"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/prowlarr.svg" height="40" alt="Prowlarr" title="Prowlarr"></a>
<a href="https://www.qbittorrent.org"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/qbittorrent.svg" height="40" alt="qBittorrent" title="qBittorrent"></a>
<a href="https://lidarr.audio"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/lidarr.svg" height="40" alt="Lidarr" title="Lidarr"></a>
<a href="https://github.com/slskd/slskd"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/slskd.svg" height="40" alt="slskd" title="slskd"></a>
<a href="https://gethomepage.dev"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/png/homepage.png" height="40" alt="Homepage" title="Homepage"></a>

<a href="https://tailscale.com"><picture><source media="(prefers-color-scheme: dark)" srcset="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/tailscale-light.svg"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/tailscale.svg" height="24" alt="Tailscale" title="Tailscale"></picture></a>
<a href="https://caddyserver.com"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/caddy.svg" height="24" alt="Caddy" title="Caddy"></a>
<a href="https://www.authelia.com"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/authelia.svg" height="24" alt="Authelia" title="Authelia"></a>
<a href="https://www.docker.com"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/docker.svg" height="24" alt="Docker" title="Docker"></a>
<a href="https://www.rust-lang.org"><picture><source media="(prefers-color-scheme: dark)" srcset="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/rust-dark.svg"><img src="https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons@83f32e08257b66fbd971ac8d7645f5b38d52e85d/svg/rust.svg" height="24" alt="Rust" title="Rust"></picture></a>

</div>

- Watch and listen on your phone or laptop, from anywhere with an internet connection
- One login for every app
- Request a show or movie from your phone: it downloads, files itself and gets subtitles
- Spotify playlists and Liked Songs sync to Navidrome, and missing songs download
- A home page with live downloads, new episodes and graphs
- One Rust binary for the sync, downloads, disk guard and metrics
- LUKS2 encryption (AES-256, Argon2id) of logins, keys and databases, behind one master password

## Quick start

Needs a Linux PC with Docker, a free Tailscale account and Rust. Install from the PC's main user account.

```sh
cp .env.example .env    # your Tailscale name and folders
./scripts/setup.sh
```

Then open `https://HOST` and sign in as `LAB_USER`.

## Features

| Feature | Turn it on |
|---|---|
| Movies, shows and music, one login, home page | Included |
| Automatic movie and show downloads | [Video downloads](#video-downloads) |
| Automatic music downloads | [Music downloads](#music-downloads) |
| Spotify playlist sync | [Spotify sync](#spotify-sync) |
| Subtitles | [Subtitles](#subtitles) |
| Weekly discovery playlist | [Weekly discovery](#weekly-discovery) |
| Jellyfin with the lab login | [Single sign-on](#single-sign-on-for-jellyfin) |
| Encryption | [Encryption](#encryption) |
| Graphs in the top bar | Included |

## Setup

`HOST`, `LAB_USER`, `TS_IP` and `DATA` below are the values from your `.env`.

### First run

`setup.sh` does it: it makes your account in Jellyfin, Navidrome and Seerr with the password you chose, gives Jellyfin its Movies and Shows libraries, and connects Seerr to Jellyfin. To change the password later, run `./scripts/set-password.sh`.

### Video downloads

1. Run `docker compose up -d sonarr radarr prowlarr qbittorrent`.
2. Run `homelab configure-video-fetch`. It connects qBittorrent, Sonarr, Radarr and Prowlarr.
3. In Prowlarr, add the indexers you want.

### Music downloads

Run `./scripts/start-music-fetch.sh`. It starts Lidarr, slskd and Soularr and checks each one.

To fetch some albums first, list them in `DATA/priority-albums.txt`, one `Artist - Album` per line.

### Spotify sync

1. Make an app at developer.spotify.com with the redirect URI `http://127.0.0.1:18080/callback`.
2. Put its `SPOTIFY_CLIENT_ID` and `SPOTIFY_CLIENT_SECRET` in `DATA/secrets/spotify.env`.
3. Run `homelab sync-spotify --auth`.
4. Run `./scripts/start-music-fetch.sh` again to turn on the daily sync.

### Subtitles

After the video downloads, run `./scripts/setup-bazarr.sh`, then `homelab homepage-secrets && docker compose up -d homepage`.

### Weekly discovery

1. In Navidrome, link your account to ListenBrainz: Personal > ListenBrainz.
2. Run `./scripts/setup-explo.sh`. A playlist of new songs then arrives every Monday.

### Single sign-on for Jellyfin

1. Let the containers reach the lab: `sudo ufw allow from 172.30.77.0/24 to TS_IP port 443 proto tcp`.
2. Run `./scripts/configure-jellyfin-sso.sh`.

### Encryption

Run `sudo ./scripts/setup-master-password.sh`. It moves the lab's data into a LUKS2 vault (AES-256, Argon2id) that one master password opens. After each reboot, unlock it with `sudo ./scripts/vault-unlock.sh`. Music and video stay outside the vault.

### Graphs

Included: `setup.sh` starts Prometheus and Grafana for the top bar graphs. When another Prometheus or Grafana already uses port 9090 or 3000, it skips them; then point that one at `127.0.0.1:9102`, serve Grafana at `/grafana` with anonymous viewers, and give its Prometheus data source the uid `prometheus`.

### Download alerts

Set `NOTIFY_CMD` in `.env`, for example to `notify-send`.

### After a code change

Run `cargo install --locked --path rust`, then `systemctl --user restart homelab-lab-metrics`.
