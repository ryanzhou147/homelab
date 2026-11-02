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

Needs a Linux PC with Docker and Tailscale, signed in, with HTTPS Certificates turned on in the Tailscale admin. Install from the PC's main user account.

```sh
./scripts/setup.sh
```

It writes `.env` with this PC's values, asks for a password, and prints the address to open.

## Features

| Feature | Turn it on |
|---|---|
| Movies, shows and music, one login, home page | Included |
| Automatic movie and show downloads | Add [indexers](#indexers) |
| Automatic music downloads | Included |
| Spotify playlist sync | [Spotify sync](#spotify-sync) |
| Subtitles | Included |
| Weekly discovery playlist | [Weekly discovery](#weekly-discovery) |
| Jellyfin with the lab login | Included |
| Encryption | [Encryption](#encryption) |
| Graphs in the top bar | Included |

## Setup

`setup.sh` installs and connects every app. These parts need your own accounts. `HOST` and `DATA` are the values in `.env`.

### Indexers

In Prowlarr at `https://HOST/prowlarr`, add the indexers you want. Shows and movies then download when you ask for them.

### Spotify sync

1. Make an app at developer.spotify.com with the redirect URI `http://127.0.0.1:18080/callback`.
2. Run `~/.cargo/bin/homelab sync-spotify --auth`, paste the app's Client ID and secret, and sign in.

### Weekly discovery

1. In Navidrome, link your account to ListenBrainz: Personal > ListenBrainz.
2. Run `./scripts/setup-explo.sh`. A playlist of new songs then arrives every Monday.

### Encryption

Run `sudo ./scripts/setup-master-password.sh`. It moves the lab's data into a LUKS2 vault (AES-256, Argon2id) that one master password opens. After each reboot, unlock it with `sudo ./scripts/vault-unlock.sh`. Music and video stay outside the vault.

### Other settings

- New password: `./scripts/set-password.sh`.
- Albums to fetch first: list them in `DATA/priority-albums.txt`, one `Artist - Album` per line.
- Download alerts: set `NOTIFY_CMD` in `.env`, for example to `notify-send`.
- Graphs when another Prometheus or Grafana holds port 9090 or 3000: point it at `127.0.0.1:9102`, serve Grafana at `/grafana` with anonymous viewers, and give its data source the uid `prometheus`.
- After a code change: `./scripts/install-homelab.sh`, then `systemctl --user restart homelab-lab-metrics`.
