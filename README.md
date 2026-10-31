<div align="center">

# homelab

A media server on one PC, reachable only over Tailscale.

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

- One login for every app, reachable only over Tailscale
- Request a show or movie from your phone: it downloads, files itself and gets subtitles
- Spotify playlists and Liked Songs sync to Navidrome, and missing songs download
- A home page with live downloads, new episodes and graphs
- One Rust binary for the sync, downloads, disk guard and metrics
- Optional encryption for all app data

## Install

Needs Linux with Docker Compose 2.30+ and user id 1000, a `/dev/dri/renderD*` GPU, Tailscale with HTTPS certificates, and Rust.

```sh
cp .env.example .env    # fill it in
./scripts/setup.sh
```

<details>
<summary>First run</summary>

1. Open `https://HOST/` and sign in as `LAB_USER`.
2. Finish the Jellyfin setup at `/video/`, with `LAB_USER` as the admin.
3. Open `/music/` once, so that Navidrome makes your account.
4. Finish the Seerr setup at `https://HOST:5055/` with your Jellyfin account.
5. Run `./scripts/set-password.sh` to give every app one password.
6. Run `homelab homepage-secrets && docker compose up -d homepage`.

</details>

<details>
<summary>Optional parts</summary>

| Part | How |
|---|---|
| Music fetchers (Lidarr, slskd, Soularr) | `./scripts/start-music-fetch.sh` |
| Spotify sync | Make an app at developer.spotify.com with the redirect `http://127.0.0.1:18080/callback`, put its `SPOTIFY_CLIENT_ID` and `SPOTIFY_CLIENT_SECRET` in `DATA/secrets/spotify.env`, run `homelab sync-spotify --auth`, then `start-music-fetch.sh` again |
| Video fetchers | `docker compose up -d sonarr radarr prowlarr qbittorrent`; sign in to qBittorrent with the password from its log, turn off its Host header check, then connect the others to it |
| Subtitles | After the video fetchers: `./scripts/setup-bazarr.sh`, then step 6 again |
| Jellyfin with the lab login | `sudo ufw allow from 172.30.77.0/24 to TS_IP port 443 proto tcp`, then `./scripts/configure-jellyfin-sso.sh` |
| Encrypted data | `sudo ./scripts/setup-master-password.sh`; after each boot, `sudo ./scripts/vault-unlock.sh` |
| Top bar graphs | Prometheus that scrapes `127.0.0.1:9102`, and Grafana on `127.0.0.1:3000` at `/grafana` with anonymous viewers and a Prometheus data source with the uid `prometheus`; without them, delete `config/homepage/custom.js` |
| Download alerts | `NOTIFY_CMD` in `.env`, e.g. `notify-send` |
| First albums | `DATA/priority-albums.txt`, one `Artist - Album` per line |

After a Rust change: `cargo install --locked --path rust`, then `systemctl --user restart homelab-lab-metrics`.

</details>
