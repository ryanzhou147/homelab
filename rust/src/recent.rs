//! Writes ${DATA}/status/recent.json: the newest Jellyfin items and the last Sonarr and Radarr
//! downloads, with episode numbers, for the home page.
//!
//! Homepage's list widget shows one raw field per line, so it cannot join "S2E3" to a title.
//! The homelab-download-status timer runs this every minute.

use crate::common::{data_dir, load_env, write_atomic};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::time::Duration;

const ITEMS: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(10);
const JELLYFIN: &str = "http://127.0.0.1:8096/video/Items?SortBy=DateCreated&SortOrder=Descending\
    &IncludeItemTypes=Episode,Movie&Recursive=true&Limit=3";
const SONARR: &str = "http://127.0.0.1:8989/sonarr/api/v3/history?pageSize=3&sortKey=date\
    &sortDirection=descending&eventType=3&includeSeries=true&includeEpisode=true";
const RADARR: &str = "http://127.0.0.1:7878/radarr/api/v3/history?pageSize=3&sortKey=date\
    &sortDirection=descending&eventType=3&includeMovie=true";

#[derive(clap::Args)]
pub struct Args {}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// "S2E3 · Title", or the title alone when a number is missing.
fn episode(season: &Value, number: &Value, title: &Value) -> String {
    match (season.as_u64(), number.as_u64()) {
        (Some(s), Some(e)) => format!("S{s}E{e} · {}", text(title)),
        _ => text(title),
    }
}

fn item(name: String, label: String) -> Value {
    json!({ "name": name, "label": label })
}

pub fn jellyfin_items(body: &Value) -> Vec<Value> {
    let items = body["Items"].as_array().into_iter().flatten().take(ITEMS);
    items
        .map(|i| match i["Type"].as_str() {
            Some("Episode") => item(
                episode(&i["ParentIndexNumber"], &i["IndexNumber"], &i["Name"]),
                text(&i["SeriesName"]),
            ),
            _ => item(text(&i["Name"]), text(&i["ProductionYear"])),
        })
        .collect()
}

pub fn sonarr_items(body: &Value) -> Vec<Value> {
    let records = body["records"].as_array().into_iter().flatten().take(ITEMS);
    records
        .map(|r| {
            let e = &r["episode"];
            item(
                episode(&e["seasonNumber"], &e["episodeNumber"], &e["title"]),
                text(&r["series"]["title"]),
            )
        })
        .collect()
}

pub fn radarr_items(body: &Value) -> Vec<Value> {
    let records = body["records"].as_array().into_iter().flatten().take(ITEMS);
    records
        .map(|r| item(text(&r["movie"]["title"]), text(&r["movie"]["year"])))
        .collect()
}

fn get(agent: &ureq::Agent, url: &str, header: (&str, &str)) -> Result<Value> {
    let mut resp = agent.get(url).header(header.0, header.1).call()?;
    Ok(resp.body_mut().read_json()?)
}

pub fn run(_: Args) -> Result<()> {
    let keys = load_env(&data_dir().join("secrets").join("homepage.env"));
    let key = |k: &str| {
        keys.get(k)
            .cloned()
            .with_context(|| format!("{k} is not in homepage.env"))
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let out_path = data_dir().join("status").join("recent.json");
    // An app that does not answer keeps its last list, so the card does not go blank.
    let old: Value = std::fs::read(&out_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}));
    let jellyfin_auth = format!(
        "MediaBrowser Token=\"{}\"",
        key("HOMEPAGE_VAR_JELLYFIN_KEY")?
    );
    let sonarr_key = key("HOMEPAGE_VAR_SONARR_KEY")?;
    let radarr_key = key("HOMEPAGE_VAR_RADARR_KEY")?;
    let lists: [(&str, Result<Vec<Value>>); 3] = [
        (
            "jellyfin",
            get(&agent, JELLYFIN, ("Authorization", &jellyfin_auth)).map(|b| jellyfin_items(&b)),
        ),
        (
            "sonarr",
            get(&agent, SONARR, ("X-Api-Key", &sonarr_key)).map(|b| sonarr_items(&b)),
        ),
        (
            "radarr",
            get(&agent, RADARR, ("X-Api-Key", &radarr_key)).map(|b| radarr_items(&b)),
        ),
    ];
    let mut out = serde_json::Map::new();
    for (app, list) in lists {
        let value = match list {
            Ok(items) => Value::from(items),
            Err(err) => {
                eprintln!("recent: {app}: {err}");
                old[app].clone()
            }
        };
        out.insert(app.to_string(), value);
    }
    std::fs::create_dir_all(out_path.parent().unwrap())?;
    write_atomic(&out_path, Value::Object(out).to_string().as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jellyfin_episodes_get_numbers_and_movies_get_years() {
        let body = json!({"Items": [
            {"Type": "Episode", "Name": "Serce", "SeriesName": "Cyberpunk: Edgerunners",
             "ParentIndexNumber": 2, "IndexNumber": 8},
            {"Type": "Episode", "Name": "Special", "SeriesName": "Show"},
            {"Type": "Movie", "Name": "Heat", "ProductionYear": 1995},
            {"Type": "Movie", "Name": "Fourth", "ProductionYear": 2000},
        ]});
        assert_eq!(
            jellyfin_items(&body),
            [
                item("S2E8 · Serce".into(), "Cyberpunk: Edgerunners".into()),
                item("Special".into(), "Show".into()),
                item("Heat".into(), "1995".into()),
            ]
        );
    }

    #[test]
    fn sonarr_and_radarr_history_lines() {
        let sonarr = json!({"records": [{"series": {"title": "Edgerunners"},
            "episode": {"seasonNumber": 2, "episodeNumber": 10, "title": "Then Everyone Went Home"}}]});
        assert_eq!(
            sonarr_items(&sonarr),
            [item(
                "S2E10 · Then Everyone Went Home".into(),
                "Edgerunners".into()
            )]
        );
        let radarr = json!({"records": [{"movie": {"title": "Heat", "year": 1995}}]});
        assert_eq!(radarr_items(&radarr), [item("Heat".into(), "1995".into())]);
        assert!(sonarr_items(&json!({})).is_empty());
    }
}
