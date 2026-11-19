<div align="center">

# homelab

Your own Netflix and Spotify, at home.

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

- Watch and listen anywhere
- One login for every app
- Request a show or movie; it downloads with subtitles
- Spotify playlists sync; paste any public link at `/playlists`
- Home page with downloads and graphs
- Encrypted logins, keys and databases

![Architecture](docs/architecture.png)

## Install

Needs Linux, Docker and Tailscale with HTTPS certificates on. Run as your main user:

```sh
./scripts/setup.sh
```

It sets up every app and three public indexers, then asks about Spotify sync, encryption and a weekly playlist. Torrents use your home IP address, with no VPN.

## Later

- **After a reboot, if encrypted:** `sudo ./scripts/vault-unlock.sh`
- **New password:** `./scripts/set-password.sh`
- **Albums first:** one `Artist - Album` per line in `DATA/priority-albums.txt` (`DATA` is in `.env`)
- **Ports 9090 or 3000 taken:** point your Prometheus at `127.0.0.1:9102` and serve Grafana at `/grafana` with anonymous viewers and data source uid `prometheus`
- **After a code change:** `./scripts/install-homelab.sh`, then `systemctl --user restart homelab-lab-metrics`
