//! The home lab's own programs in one binary: `homelab <command>`.

mod common;
mod configure;
mod configure_video;
mod discover;
mod disk_guard;
mod exported;
mod fetch_missing;
mod fill_loop;
mod homepage_secrets;
mod import;
mod lidarr;
mod locked_alert;
mod merge;
mod merge_id3;
mod merge_tags;
mod metrics;
mod navidrome;
mod online;
mod playlist;
mod playlist_web;
mod recent;
mod soulseek;
mod spotdl_album;
mod spotify;
mod status;
mod sync;
mod text;
mod unlock_prompt;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "homelab", about = "The home lab's own programs")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Copy Spotify playlists and Liked Songs to Navidrome; queue missing albums in Lidarr.
    SyncSpotify(sync::Args),
    /// Rebuild Exportify CSV playlists as Navidrome playlists.
    ImportPlaylists(import::Args),
    /// Write a spotdl list of the Exportify songs that Navidrome lacks.
    ExportedMissing(exported::Args),
    /// Merge albums that spotdl and Lidarr split in Navidrome.
    MergeAlbums(merge::Args),
    /// Serve Prometheus metrics for the home lab apps on 127.0.0.1:9102.
    Metrics(metrics::Args),
    /// Write the spotdl progress file for the home page.
    DownloadStatus(status::Args),
    /// Write the newest shows and movies, with episode numbers, for the home page.
    Recent(recent::Args),
    /// Write the home page's API keys to homepage.env.
    HomepageSecrets(homepage_secrets::Args),
    /// Download one album with spotdl.
    SpotdlAlbum(spotdl_album::Args),
    /// Configure Lidarr for the music fetchers and check each piece.
    ConfigureMusicFetch(configure::Args),
    /// Wire qBittorrent, Sonarr, Radarr and Prowlarr together.
    ConfigureVideoFetch(configure_video::Args),
    /// Download playlist songs until all are in the library.
    FillLoop(fill_loop::Args),
    /// Stop the music downloads when the media disk is low.
    DiskGuard(disk_guard::Args),
    /// Alert NOTIFY_CMD when the lab's vault is still locked after a boot.
    LockedAlert(locked_alert::Args),
    /// Open a terminal that unlocks the lab when you come back to the desktop and it is locked.
    UnlockPrompt(unlock_prompt::Args),
    /// Get this week's ListenBrainz recommendations into Navidrome with Explo.
    Discover(discover::Args),
    /// Add a Spotify playlist (public link or Exportify CSV) to Navidrome, fetching missing songs.
    AddPlaylist(playlist::Args),
    /// Serve the Playlists page on 127.0.0.1:9103 for the lab at /playlists.
    PlaylistWeb(playlist_web::Args),
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::SyncSpotify(a) => sync::run(a),
        Command::ImportPlaylists(a) => import::run(a),
        Command::ExportedMissing(a) => exported::run(a),
        Command::MergeAlbums(a) => merge::run(a),
        Command::Metrics(a) => metrics::run(a),
        Command::DownloadStatus(a) => status::run(a),
        Command::Recent(a) => recent::run(a),
        Command::HomepageSecrets(a) => homepage_secrets::run(a),
        Command::SpotdlAlbum(a) => spotdl_album::run(a),
        Command::ConfigureMusicFetch(a) => configure::run(a),
        Command::ConfigureVideoFetch(a) => configure_video::run(a),
        Command::FillLoop(a) => fill_loop::run(a),
        Command::DiskGuard(a) => disk_guard::run(a),
        Command::LockedAlert(a) => locked_alert::run(a),
        Command::UnlockPrompt(a) => unlock_prompt::run(a),
        Command::Discover(a) => discover::run(a),
        Command::AddPlaylist(a) => playlist::run(a),
        Command::PlaylistWeb(a) => playlist_web::run(a),
    }
}
