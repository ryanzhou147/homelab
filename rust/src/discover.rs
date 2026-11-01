//! Gets this week's ListenBrainz music recommendations into Navidrome: Explo downloads them into
//! MEDIA/music/explo and makes a playlist of them. The homelab-discover timer runs it each Monday;
//! scripts/setup-explo.sh writes its settings first.
//!
//! ListenBrainz's own Weekly Exploration playlist has 50 songs, but Explo's API mode gets only 25
//! (it asks without a count). So the playlist is used when ListenBrainz made one this week.

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::common::{data_dir, load_env, repo_dir};

const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(clap::Args)]
pub struct Args {}

/// Days since 1970-01-01 for a "YYYY-MM-DD..." date (Howard Hinnant's days_from_civil).
fn days(date: &str) -> Option<i64> {
    let mut parts = date.get(..10)?.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (parts.next()??, parts.next()??, parts.next()??);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    Some(era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468)
}

/// This week's Weekly Exploration, found as Explo finds it (by its source_patch). ListenBrainz
/// makes it on Sunday evening, so at the Monday run it is at most 2 days old. It keeps old ones and
/// sometimes makes one a day late, so last week's can be 6 days old; it would bring old songs back.
fn has_weekly_exploration(created_for: &Value, today: i64) -> bool {
    created_for["playlists"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| &p["playlist"])
        .filter(|p| {
            p["extension"]["https://musicbrainz.org/doc/jspf#playlist"]["additional_metadata"]
                ["algorithm_metadata"]["source_patch"]
                == "weekly-exploration"
        })
        .filter_map(|p| p["date"].as_str().and_then(days))
        .any(|made| today - made < 3)
}

fn weekly_exploration_exists(user: &str) -> Result<bool> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let url = format!("https://api.listenbrainz.org/1/user/{user}/playlists/createdfor");
    let body: Value = agent.get(&url).call()?.body_mut().read_json()?;
    let today = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64
        / 86_400;
    Ok(has_weekly_exploration(&body, today))
}

pub fn run(_args: Args) -> Result<()> {
    let settings = load_env(&data_dir().join("explo").join(".env"));
    let user = settings
        .get("LISTENBRAINZ_USER")
        .map(|u| u.trim_matches('\'').to_string())
        .context("No LISTENBRAINZ_USER in DATA/explo/.env: run scripts/setup-explo.sh")?;
    let mode = match weekly_exploration_exists(&user) {
        Ok(true) => "playlist",
        Ok(false) => "api",
        Err(err) => {
            eprintln!("discover: ListenBrainz playlists: {err}; using the 25 recommendations");
            "api"
        }
    };
    println!("discover: LISTENBRAINZ_DISCOVERY={mode}");
    let status = Command::new("docker")
        .args(["compose", "--profile", "discovery", "run", "--rm"])
        .args(["-e", &format!("LISTENBRAINZ_DISCOVERY={mode}"), "explo"])
        .current_dir(repo_dir())
        .status()?;
    ensure!(status.success(), "explo failed: {status}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn playlist(patch: &str, date: &str) -> Value {
        json!({"playlist": {"date": date, "extension": {"https://musicbrainz.org/doc/jspf#playlist":
            {"additional_metadata": {"algorithm_metadata": {"source_patch": patch}}}}}})
    }

    #[test]
    fn days_counts_from_1970() {
        assert_eq!(days("1970-01-01"), Some(0));
        assert_eq!(days("2000-03-01T00:00:00+00:00"), Some(11_017));
        assert_eq!(days("2026-11-02"), Some(20_759));
        assert_eq!(days("garbage"), None);
    }

    #[test]
    fn finds_only_a_fresh_weekly_exploration() {
        let today = days("2026-11-03").unwrap();
        let fresh = json!({"playlists": [playlist("weekly-jams", "2026-11-02T06:00:00+00:00"),
            playlist("weekly-exploration", "2026-11-02T06:00:00+00:00")]});
        assert!(has_weekly_exploration(&fresh, today));
        let last_week_late = json!({"playlists": [playlist("weekly-exploration", "2026-10-28T12:24:00+00:00")]});
        assert!(!has_weekly_exploration(&last_week_late, today));
        let stale = json!({"playlists": [playlist("weekly-exploration", "2026-08-24T12:24:00+00:00")]});
        assert!(!has_weekly_exploration(&stale, today));
        let jams = json!({"playlists": [playlist("weekly-jams", "2026-11-02T06:00:00+00:00")]});
        assert!(!has_weekly_exploration(&jams, today));
        assert!(!has_weekly_exploration(&json!({"count": 0, "playlists": []}), today));
        assert!(!has_weekly_exploration(&json!({}), today));
    }
}
