//! Runs until all playlist songs are in the library:
//! 1. Downloads albums Soularr failed on via spotdl (using Spotify album URLs)
//! 2. Rescans Navidrome
//! 3. Updates Spotify playlists and downloads each missing track once with spotdl
//!
//! Run in background: setsid nohup homelab fill-loop >> ~/homelab-data/music-fill.log 2>&1 &

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Result, ensure};
use regex::Regex;
use serde_json::Value;

use crate::common::{data_dir, home, load_env};
use crate::disk_guard::df_avail;

const STOP_BELOW_GB: u64 = 5;
const DOWNLOAD_FROM_GB: u64 = 15;
const MAX_SPOTDL_FAILS: u32 = 3;
const LIDARR: &str = "http://localhost:8686/lidarr";
const SCAN_POLLS: u32 = 30;

/// Albums that Soularr struggles with, ordered by playlist impact.
const PRIORITY_ALBUMS: &[(&str, &str)] = &[
    ("Kendrick Lamar", "DAMN."),
    ("Don Toliver", "HARDSTONE PSYCHO"),
    ("Don Toliver", "Love Sick"),
    ("Don Toliver", "Life of a DON"),
    ("Kanye West", "The Life of Pablo"),
    ("Kanye West", "My Beautiful Dark Twisted Fantasy"),
    ("Kanye West", "808s & Heartbreak"),
    ("Kanye West", "Late Registration"),
    ("Baby Keem", "The Melodic Blue"),
    ("A$AP Rocky", "AT.LONG.LAST.A$AP"),
    ("J. Cole", "Born Sinner"),
    ("J. Cole", "The Off-Season"),
    ("SZA", "SOS"),
    ("Playboi Carti", "MUSIC"),
    ("Playboi Carti", "Whole Lotta Red"),
    ("Frank Ocean", "Blonde"),
    ("Mac Miller", "Swimming"),
    ("Lil Uzi Vert", "Eternal Atake"),
    ("Lil Uzi Vert", "Luv Is Rage 2"),
    ("Vince Staples", "Dark Times"),
    ("Kid Cudi", "INSANO"),
    ("Kid Cudi", "Man on the Moon: The End of Day"),
    ("Gunna", "One of Wun"),
    ("French Montana", "Jungle Rules"),
    ("Lil Wayne", "Tha Carter III"),
    ("Lil Wayne", "Tha Carter IV"),
    ("Lil Wayne", "Tha Carter V"),
    ("JAY-Z", "Reasonable Doubt"),
    ("JAY-Z", "Watch the Throne"),
    ("Nas", "Illmatic"),
    ("Nas", "Hip Hop Is Dead"),
    ("Eminem", "The Eminem Show"),
    ("Eminem", "Relapse"),
    ("2Pac", "Me Against the World"),
    ("2Pac", "Until the End of Time"),
    ("The Notorious B.I.G.", "Life After Death"),
    ("OutKast", "Aquemini"),
    ("MF DOOM", "MM..FOOD"),
    ("Earl Sweatshirt", "Doris"),
    ("Pusha T", "My Name Is My Name"),
    ("Childish Gambino", "Camp"),
    ("50 Cent", "Get Rich or Die Tryin'"),
    ("Drake", "Scorpion"),
    ("Swedish House Mafia", "Paradise Again"),
    ("EKKSTACY", "Negative"),
    ("Joji", "Nectar"),
    ("Joji", "BALLADS 1"),
    ("Internet Money", "B4 the Storm"),
    ("Molchat Doma", "Этажи"),
    ("Hozier", "Hozier"),
    ("Tame Impala", "Currents"),
    ("Daft Punk", "Random Access Memories"),
    ("Gorillaz", "Cracker Island"),
];

#[derive(clap::Args)]
pub struct Args {}

/// The outside world of the loop: programs, HTTP calls and the clock. Tests fake it.
trait Lab {
    fn free_gb(&mut self) -> Result<u64>;
    /// `homelab spotdl-album`, output to the log; true when it exits 0.
    fn spotdl_album(&mut self, artist: &str, album: &str) -> bool;
    /// Monitored albums that Lidarr still wants, as (artist, title).
    fn wanted_albums(&mut self) -> Vec<(String, String)>;
    fn merge_albums(&mut self);
    fn navidrome_scan(&mut self);
    fn navidrome_count(&mut self) -> String;
    /// `homelab sync-spotify --missing-out`, which writes the missing list.
    fn spotify_sync(&mut self);
    fn lidarr_still_wants(&mut self) -> String;
    /// spotdl on the missing list; true when it exits 0.
    fn spotdl_missing(&mut self) -> bool;
    fn sleep_between_rounds(&mut self);
}

/// The loop's own files under $DATA.
struct Files {
    log: PathBuf,
    done: PathBuf,
    missing: PathBuf,
    tried: PathBuf,
}

impl Files {
    fn new(data: &Path) -> Self {
        Files {
            log: data.join("music-fill.log"),
            done: data.join("spotdl-done.txt"),
            missing: data.join("missing.spotdl"),
            tried: data.join("spotdl-tried.txt"),
        }
    }

    fn append(&self, path: &Path, text: &str) -> Result<()> {
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(f.write_all(text.as_bytes())?)
    }

    /// `echo "[HH:MM:SS] msg" | tee -a $LOG`.
    fn log(&self, msg: &str) -> Result<()> {
        let line = format!("[{}] {msg}\n", clock());
        print!("{line}");
        self.append(&self.log, &line)
    }
}

fn clock() -> String {
    // date(1) gives local time with no time zone crate.
    Command::new("date")
        .arg("+%H:%M:%S")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn run(_args: Args) -> Result<()> {
    let data = data_dir();
    let mut lab = LiveLab::new(&data)?;
    let count = fill(&mut lab, &Files::new(&data))?;
    let notify = home().join(".claude/harness/notify.py");
    let message = format!("Music fill loop done: Navidrome has {count} tracks, playlists updated");
    let _ = Command::new(notify)
        .arg(message)
        .stderr(Stdio::null())
        .status();
    Ok(())
}

/// The rounds of the loop; returns the last Navidrome track count.
fn fill(lab: &mut impl Lab, files: &Files) -> Result<String> {
    files.append(&files.done, "")?;
    files.log("=== music-fill-loop started (round 1) ===")?;
    let mut count = "0".to_string();
    let mut spotdl_fails = 0;
    for round in 1.. {
        files.log(&format!("--- round {round} ---"))?;
        let free_gb = lab.free_gb()?;
        files.log(&format!("Disk free: {free_gb}GB"))?;
        if free_gb < STOP_BELOW_GB {
            files.log(&format!("Disk below {STOP_BELOW_GB}GB, stopping"))?;
            break;
        }
        if free_gb >= DOWNLOAD_FROM_GB {
            // Priority downloads via spotdl.
            for (artist, album) in PRIORITY_ALBUMS {
                download_album(lab, files, artist, album)?;
            }
            // Also pull remaining wanted albums from Lidarr.
            for (artist, album) in lab.wanted_albums() {
                download_album(lab, files, &artist, &album)?;
            }
        }

        // spotdl and Lidarr files in one folder show as two albums. The merge uses the last scan,
        // and this scan picks up its moves before the playlist sync.
        lab.merge_albums();
        files.log("Scanning Navidrome...")?;
        lab.navidrome_scan();
        count = lab.navidrome_count();
        files.log(&format!("Navidrome: {count} tracks"))?;

        files.log("Syncing playlists...")?;
        // A failed sync then leaves no stale list for spotdl to retry.
        if let Err(err) = std::fs::remove_file(&files.missing) {
            ensure!(err.kind() == std::io::ErrorKind::NotFound, err);
        }
        lab.spotify_sync();

        files.log(&format!(
            "Lidarr still wants: {} albums",
            lab.lidarr_still_wants()
        ))?;
        if missing_count(&files.missing) == Some(0) {
            files.log("Every missing track was tried once. Loop done.")?;
            break;
        }
        if free_gb >= DOWNLOAD_FROM_GB {
            download_missing(lab, files, &mut spotdl_fails)?;
        }
        if spotdl_fails >= MAX_SPOTDL_FAILS {
            files.log(&format!(
                "spotdl failed {spotdl_fails} rounds in a row. Loop stopped."
            ))?;
            break;
        }

        files.log("Sleeping 5 minutes before next round...")?;
        lab.sleep_between_rounds();
    }
    files.log("=== music-fill-loop complete ===")?;
    Ok(count)
}

fn download_album(lab: &mut impl Lab, files: &Files, artist: &str, album: &str) -> Result<()> {
    let key = format!("{artist}|||{album}");
    // Only skip if a previous spotdl run completed fully for this album.
    let done = std::fs::read_to_string(&files.done).unwrap_or_default();
    if done.lines().any(|l| l == key) {
        return Ok(());
    }
    files.log(&format!("Downloading: {artist} - {album}"))?;
    // spotdl skips tracks already on disk (--overwrite skip is default).
    if lab.spotdl_album(artist, album) {
        files.append(&files.done, &format!("{key}\n"))
    } else {
        files.log(&format!("Failed: {artist} - {album}"))
    }
}

/// URLs in the missing list; a missing or broken file gives none.
fn missing_urls(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let songs: Vec<Value> = serde_json::from_str(&text).unwrap_or_default();
    let urls: Option<Vec<String>> = songs
        .iter()
        .map(|s| s["url"].as_str().map(String::from))
        .collect();
    urls.unwrap_or_default()
}

/// Songs in the missing list, None when it is missing or broken.
fn missing_count(path: &Path) -> Option<usize> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Vec<Value>>(&text)
        .ok()
        .map(|v| v.len())
}

// Each track gets one try: spotdl finds no match for some, and retrying them every round wastes hours.
fn download_missing(lab: &mut impl Lab, files: &Files, fails: &mut u32) -> Result<()> {
    let urls = missing_urls(&files.missing);
    if urls.is_empty() {
        return Ok(());
    }
    files.log(&format!(
        "spotdl: downloading {} missing tracks",
        urls.len()
    ))?;
    // spotdl exits 0 when only some tracks fail, so a non-zero exit means nothing was really tried.
    if lab.spotdl_missing() {
        files.append(&files.tried, &format!("{}\n", urls.join("\n")))?;
        *fails = 0;
    } else {
        *fails += 1;
        files.log(&format!(
            "spotdl exited with an error ({fails} in a row); these tracks stay untried"
        ))?;
    }
    Ok(())
}

fn spotdl_missing_args(missing: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec!["download".into(), missing.into()];
    args.extend(
        [
            "--format",
            "mp3",
            "--bitrate",
            "320k",
            "--threads",
            "4",
            "--output",
            "{album-artist}/{album} ({year})/{track-number} - {title}.{output-ext}",
        ]
        .map(Into::into),
    );
    args
}

/// Monitored albums from Lidarr's wanted/missing page, as the shell script's inline Python reads them.
fn parse_wanted(data: &Value) -> Vec<(String, String)> {
    let records = data["records"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    records
        .iter()
        .filter_map(|r| {
            let artist = r["artist"]["artistName"].as_str().unwrap_or("");
            let title = r["title"].as_str().unwrap_or("");
            (!artist.is_empty() && !title.is_empty())
                .then(|| (artist.to_string(), title.to_string()))
        })
        .collect()
}

/// A JSON value as Python's print() shows an int or string, for the log.
fn shown(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), String::from)
}

struct LiveLab {
    data: PathBuf,
    music: PathBuf,
    missing: PathBuf,
    log: PathBuf,
    navidrome_url: String,
    navidrome_user: String,
    navidrome_password: String,
    lidarr_key: String,
    /// The env files' keys, given to each child as `set -a; source` does.
    secrets: HashMap<String, String>,
    exe: PathBuf,
}

impl LiveLab {
    fn new(data: &Path) -> Result<Self> {
        let mut secrets = HashMap::new();
        for name in ["navidrome.env", "spotify.env", "lidarr.env"] {
            let path = data.join("secrets").join(name);
            ensure!(path.exists(), "{} does not exist", path.display());
            secrets.extend(load_env(&path));
        }
        // An env file value wins over the environment, as `source` after the defaults does.
        let var = |k: &str, default: &str| {
            secrets
                .get(k)
                .cloned()
                .or_else(|| std::env::var(k).ok().filter(|v| !v.is_empty()))
                .unwrap_or_else(|| default.to_string())
        };
        let lidarr_key = var("LIDARR__AUTH__APIKEY", "");
        ensure!(
            !lidarr_key.is_empty(),
            "Missing LIDARR__AUTH__APIKEY in lidarr.env"
        );
        Ok(LiveLab {
            data: data.to_path_buf(),
            music: home().join("homelab-media").join("music"),
            missing: data.join("missing.spotdl"),
            log: data.join("music-fill.log"),
            navidrome_url: var("NAVIDROME_URL", "https://your-pc.your-tailnet.ts.net/music"),
            navidrome_user: var("NAVIDROME_USER", "admin"),
            navidrome_password: var("NAVIDROME_PASSWORD", ""),
            lidarr_key,
            exe: std::env::current_exe()?,
            secrets,
        })
    }

    fn command(&self, program: &Path) -> Command {
        let mut c = Command::new(program);
        c.envs(&self.secrets);
        c
    }

    /// Runs `cmd` with stdout and stderr appended to the log; true when it exits 0.
    fn to_log(&self, mut cmd: Command) -> bool {
        let mut run = || -> Result<bool> {
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log)?;
            Ok(cmd.stdout(log.try_clone()?).stderr(log).status()?.success())
        };
        run().unwrap_or(false)
    }

    /// `cmd 2>&1 | grep -E pattern | tee -a $LOG`; errors are ignored, as `|| true` does.
    fn tee(&self, mut cmd: Command, keep: Option<&Regex>) {
        let run = || -> Result<()> {
            let (reader, writer) = std::io::pipe()?;
            cmd.stdout(writer.try_clone()?).stderr(writer);
            let mut child = cmd.spawn()?;
            drop(cmd); // closes our copies of the pipe, so the read ends with the child
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log)?;
            for line in BufReader::new(reader).lines() {
                let line = line?;
                if keep.is_none_or(|re| re.is_match(&line)) {
                    println!("{line}");
                    writeln!(log, "{line}")?;
                }
            }
            child.wait()?;
            Ok(())
        };
        if let Err(err) = run() {
            eprintln!("{err:#}");
        }
    }

    fn navidrome_status(&self, view: &str) -> Result<Value> {
        let query = crate::text::urlencode(&[
            ("u", &self.navidrome_user),
            ("p", &self.navidrome_password),
            ("v", "1.16.1"),
            ("c", "fill-loop"),
            ("f", "json"),
        ]);
        let url = format!("{}/rest/{view}.view?{query}", self.navidrome_url);
        Ok(http().get(&url).call()?.body_mut().read_json()?)
    }

    fn lidarr_wanted(&self, page_size: u32) -> Result<Value> {
        let url = format!("{LIDARR}/api/v1/wanted/missing?pageSize={page_size}&monitored=true");
        Ok(http()
            .get(&url)
            .header("X-Api-Key", &self.lidarr_key)
            .call()?
            .body_mut()
            .read_json()?)
    }
}

/// 30 s timeout, so a hung Navidrome or Lidarr cannot stop the loop; HTTP errors stay errors,
/// like the script's `curl -f`.
fn http() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into()
}

impl Lab for LiveLab {
    fn free_gb(&mut self) -> Result<u64> {
        df_avail(&self.music, "-BG")
    }

    fn spotdl_album(&mut self, artist: &str, album: &str) -> bool {
        let mut cmd = self.command(&self.exe);
        cmd.args(["spotdl-album", artist, album])
            .env("DATA", &self.data);
        self.to_log(cmd)
    }

    fn wanted_albums(&mut self) -> Vec<(String, String)> {
        self.lidarr_wanted(200)
            .map(|d| parse_wanted(&d))
            .unwrap_or_default()
    }

    fn merge_albums(&mut self) {
        let mut cmd = self.command(&self.exe);
        cmd.args(["merge-albums", "--apply"]);
        self.tee(cmd, None);
    }

    fn navidrome_scan(&mut self) {
        let _ = self.navidrome_status("startScan");
        // Wait for the scan to complete (poll until scanning is false).
        // The shell script printed Python's True, so its loop never waited past one poll.
        for _ in 0..SCAN_POLLS {
            std::thread::sleep(Duration::from_secs(5));
            let scanning = self.navidrome_status("getScanStatus").ok().and_then(|d| {
                d.pointer("/subsonic-response/scanStatus/scanning")
                    .and_then(Value::as_bool)
            });
            if scanning != Some(true) {
                break;
            }
        }
    }

    fn navidrome_count(&mut self) -> String {
        match self.navidrome_status("getScanStatus") {
            Ok(d) => d
                .pointer("/subsonic-response/scanStatus/count")
                .map_or("0".into(), shown),
            Err(_) => "0".into(),
        }
    }

    fn spotify_sync(&mut self) {
        let mut cmd = self.command(&self.exe);
        cmd.arg("sync-spotify")
            .arg("--missing-out")
            .arg(&self.missing)
            .env("DATA", &self.data)
            .env("NAVIDROME_URL", &self.navidrome_url)
            .env("NAVIDROME_USER", &self.navidrome_user)
            .env("NAVIDROME_PASSWORD", &self.navidrome_password);
        let keep = Regex::new("in library|^   [0-9]|rate limit|missing tracks written")
            .expect("valid regex");
        self.tee(cmd, Some(&keep));
    }

    fn lidarr_still_wants(&mut self) -> String {
        match self.lidarr_wanted(1) {
            Ok(d) => d.get("totalRecords").map_or("0".into(), shown),
            Err(_) => "?".into(),
        }
    }

    fn spotdl_missing(&mut self) -> bool {
        let mut cmd = self.command(Path::new("spotdl"));
        cmd.args(spotdl_missing_args(&self.missing))
            .current_dir(&self.music);
        self.to_log(cmd)
    }

    fn sleep_between_rounds(&mut self) {
        std::thread::sleep(Duration::from_secs(300));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records each step; `rounds` gives free GB, the sync's missing list and spotdl's result.
    struct FakeLab {
        steps: Vec<String>,
        rounds: Vec<(u64, Option<&'static str>, bool)>,
        round: usize,
        missing: PathBuf,
        failing_albums: Vec<&'static str>,
    }

    impl FakeLab {
        fn new(dir: &Path, rounds: Vec<(u64, Option<&'static str>, bool)>) -> Self {
            FakeLab {
                steps: vec![],
                rounds,
                round: 0,
                missing: dir.join("missing.spotdl"),
                failing_albums: vec![],
            }
        }
        fn now(&self) -> (u64, Option<&'static str>, bool) {
            self.rounds[(self.round - 1).min(self.rounds.len() - 1)]
        }
    }

    impl Lab for FakeLab {
        fn free_gb(&mut self) -> Result<u64> {
            self.round += 1;
            self.steps.push("df".into());
            Ok(self.now().0)
        }
        fn spotdl_album(&mut self, _artist: &str, album: &str) -> bool {
            self.steps.push(format!("album {album}"));
            !self.failing_albums.contains(&album)
        }
        fn wanted_albums(&mut self) -> Vec<(String, String)> {
            self.steps.push("wanted".into());
            vec![("Lidarr Artist".into(), "Wanted One".into())]
        }
        fn merge_albums(&mut self) {
            self.steps.push("merge".into());
        }
        fn navidrome_scan(&mut self) {
            self.steps.push("scan".into());
        }
        fn navidrome_count(&mut self) -> String {
            self.steps.push("count".into());
            "1234".into()
        }
        fn spotify_sync(&mut self) {
            self.steps.push("sync".into());
            assert!(
                !self.missing.exists(),
                "the old missing list is gone before the sync"
            );
            if let Some(list) = self.now().1 {
                std::fs::write(&self.missing, list).unwrap();
            }
        }
        fn lidarr_still_wants(&mut self) -> String {
            self.steps.push("still".into());
            "7".into()
        }
        fn spotdl_missing(&mut self) -> bool {
            self.steps.push("spotdl".into());
            self.now().2
        }
        fn sleep_between_rounds(&mut self) {
            self.steps.push("sleep".into());
        }
    }

    fn setup() -> (tempfile::TempDir, Files) {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(dir.path());
        (dir, files)
    }

    const TWO: Option<&str> = Some(r#"[{"url": "u1"}, {"url": "u2"}]"#);
    const NONE_LEFT: Option<&str> = Some("[]");

    #[test]
    fn one_round_runs_every_step_in_order() {
        let (dir, files) = setup();
        std::fs::write(&files.done, "Kendrick Lamar|||DAMN.\n").unwrap();
        let mut lab = FakeLab::new(dir.path(), vec![(100, TWO, true), (100, NONE_LEFT, true)]);
        lab.failing_albums = vec!["SOS"];
        let count = fill(&mut lab, &files).unwrap();
        assert_eq!(count, "1234");

        let albums = PRIORITY_ALBUMS.len() - 1; // DAMN. is done
        let round1 = &lab.steps[..albums + 10];
        assert_eq!(round1[0], "df");
        assert_eq!(round1[1], "album HARDSTONE PSYCHO");
        assert_eq!(round1[albums], "album Cracker Island");
        let tail = [
            "wanted",
            "album Wanted One",
            "merge",
            "scan",
            "count",
            "sync",
            "still",
            "spotdl",
            "sleep",
        ];
        assert_eq!(&round1[albums + 1..], tail);
        // Round 2: done albums are skipped, SOS failed so it is tried again, then the loop ends.
        let round2 = &lab.steps[albums + 10..];
        let want2 = [
            "df",
            "album SOS",
            "wanted",
            "merge",
            "scan",
            "count",
            "sync",
            "still",
        ];
        assert_eq!(round2, want2);

        let done = std::fs::read_to_string(&files.done).unwrap();
        assert!(done.contains("Lidarr Artist|||Wanted One\n") && !done.contains("SOS"));
        assert_eq!(std::fs::read_to_string(&files.tried).unwrap(), "u1\nu2\n");
        let log = std::fs::read_to_string(&files.log).unwrap();
        for want in [
            "] === music-fill-loop started (round 1) ===\n",
            "] Disk free: 100GB\n",
            "] Failed: SZA - SOS\n",
            "] spotdl: downloading 2 missing tracks\n",
            "] Navidrome: 1234 tracks\n",
            "] Lidarr still wants: 7 albums\n",
            "] Every missing track was tried once. Loop done.\n",
            "] === music-fill-loop complete ===\n",
        ] {
            assert!(log.contains(want), "log lacks {want:?}:\n{log}");
        }
    }

    #[test]
    fn stops_below_5_gb_and_skips_downloads_below_15() {
        let (dir, files) = setup();
        let mut lab = FakeLab::new(dir.path(), vec![(14, TWO, true), (4, None, true)]);
        fill(&mut lab, &files).unwrap();
        let want = [
            "df", "merge", "scan", "count", "sync", "still", "sleep", "df",
        ];
        assert_eq!(lab.steps, want);
        assert!(!files.tried.exists());
        let log = std::fs::read_to_string(&files.log).unwrap();
        assert!(log.contains("] Disk below 5GB, stopping\n"));
    }

    #[test]
    fn three_spotdl_failures_in_a_row_stop_the_loop() {
        let (dir, files) = setup();
        let rounds = vec![(100, TWO, false), (100, TWO, true), (100, TWO, false)];
        let mut lab = FakeLab::new(dir.path(), [rounds, vec![(100, TWO, false); 3]].concat());
        std::fs::write(
            &files.done,
            PRIORITY_ALBUMS
                .iter()
                .map(|(a, b)| format!("{a}|||{b}\n"))
                .collect::<String>(),
        )
        .unwrap();
        fill(&mut lab, &files).unwrap();
        // The success in round 2 resets the count, so rounds 3, 4 and 5 fail in a row.
        assert_eq!(lab.steps.iter().filter(|s| *s == "df").count(), 5);
        assert_eq!(lab.steps.last().unwrap(), "spotdl");
        assert_eq!(std::fs::read_to_string(&files.tried).unwrap(), "u1\nu2\n");
        let log = std::fs::read_to_string(&files.log).unwrap();
        assert!(
            log.contains("] spotdl exited with an error (2 in a row); these tracks stay untried\n")
        );
        assert!(log.contains("] spotdl failed 3 rounds in a row. Loop stopped.\n"));
    }

    #[test]
    fn a_failed_sync_leaves_no_list_and_no_spotdl_run() {
        let (dir, files) = setup();
        std::fs::write(&files.missing, TWO.unwrap()).unwrap(); // stale list from an earlier round
        let mut lab = FakeLab::new(
            dir.path(),
            vec![(14, None, true), (100, None, true), (1, None, true)],
        );
        std::fs::write(
            &files.done,
            PRIORITY_ALBUMS
                .iter()
                .map(|(a, b)| format!("{a}|||{b}\n"))
                .collect::<String>(),
        )
        .unwrap();
        fill(&mut lab, &files).unwrap();
        assert!(!lab.steps.contains(&"spotdl".to_string()));
        assert!(!files.missing.exists());
    }

    #[test]
    fn spotdl_missing_command_line_matches_the_shell_script() {
        let args: Vec<String> = spotdl_missing_args(Path::new("/d/missing.spotdl"))
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        assert_eq!(
            args.join(" "),
            "download /d/missing.spotdl --format mp3 --bitrate 320k --threads 4 \
             --output {album-artist}/{album} ({year})/{track-number} - {title}.{output-ext}"
        );
    }

    #[test]
    fn wanted_keeps_records_with_artist_and_title() {
        let data = serde_json::json!({"records": [
            {"artist": {"artistName": "A"}, "title": "T"},
            {"artist": {"artistName": ""}, "title": "T"},
            {"title": "T"},
            {"artist": {"artistName": "B"}},
        ]});
        assert_eq!(
            parse_wanted(&data),
            vec![("A".to_string(), "T".to_string())]
        );
        assert!(parse_wanted(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn missing_list_reading() {
        let (_dir, files) = setup();
        assert_eq!(missing_count(&files.missing), None);
        assert!(missing_urls(&files.missing).is_empty());
        std::fs::write(&files.missing, "not json").unwrap();
        assert_eq!(missing_count(&files.missing), None);
        std::fs::write(&files.missing, TWO.unwrap()).unwrap();
        assert_eq!(missing_count(&files.missing), Some(2));
        assert_eq!(missing_urls(&files.missing), ["u1", "u2"]);
    }

    #[test]
    fn priority_list_matches_the_shell_script() {
        let script = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scripts/music-fill-loop.sh"
        ));
        let Ok(script) = script else { return }; // the script goes away at the switch-over
        let block = script
            .split("PRIORITY_ALBUMS=(")
            .nth(1)
            .unwrap()
            .split("\n)")
            .next()
            .unwrap();
        let from_script: Vec<String> = block
            .lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix('"')?
                    .strip_suffix('"')
                    .map(|s| s.replace("\\$", "$"))
            })
            .collect();
        let ours: Vec<String> = PRIORITY_ALBUMS
            .iter()
            .map(|(a, b)| format!("{a}|{b}"))
            .collect();
        assert_eq!(ours, from_script);
    }
}
