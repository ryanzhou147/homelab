//! Wires the video download apps together, so a new install needs no clicks: qBittorrent's web
//! settings, Sonarr's and Radarr's download client and library folder, and Prowlarr's links to
//! both. The indexers stay the user's choice, in Prowlarr. Safe to run again: it adds only what is
//! missing, and warns about what exists but differs.
//!
//!     homelab configure-video-fetch
//!
//! A new qBittorrent prints a one-time admin password in its log; the first run uses it to turn
//! on "no login from this machine", which the other apps then rely on. If qBittorrent has a real
//! password, give it in the QBITTORRENT_PASSWORD environment variable.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::{Value, json};

use crate::common::{data_dir, repo_dir};
use crate::homepage_secrets::arr_key;

const TIMEOUT: Duration = Duration::from_secs(20);
const QBIT_INSIDE: (&str, u16) = ("localhost", 8080); // as Sonarr and Radarr see it
const PROWLARR_INSIDE: &str = "http://localhost:9696/prowlarr";
const QBIT_PREFS: &str = r#"{"bypass_local_auth": true, "web_ui_host_header_validation_enabled": false,
    "web_ui_address": "127.0.0.1", "save_path": "/data/downloads/qbittorrent"}"#;

#[derive(clap::Args)]
pub struct Args {
    /// qBittorrent's web UI, as this machine reaches it (through the fetch gate).
    #[arg(long, default_value = "http://127.0.0.1:8081")]
    qbittorrent: String,
    #[arg(long, default_value = "http://127.0.0.1:8989/sonarr")]
    sonarr: String,
    #[arg(long, default_value = "http://127.0.0.1:7878/radarr")]
    radarr: String,
    #[arg(long, default_value = "http://127.0.0.1:9696/prowlarr")]
    prowlarr: String,
}

/// One arr app: its name in DATA, its address from this machine and from inside the fetch gate's
/// network (where Prowlarr calls it), the category field and value it gives qBittorrent, and its
/// library folder.
struct Arr {
    app: &'static str,
    url: String,
    inside: &'static str,
    category: (&'static str, &'static str),
    root: &'static str,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into()
}

fn send(agent: &ureq::Agent, method: &str, url: &str, key: &str, body: Option<&Value>) -> Result<Value> {
    let req = ureq::http::Request::builder()
        .method(method)
        .uri(url)
        .header("X-Api-Key", key)
        .header("Content-Type", "application/json")
        .body(body.map(Value::to_string).unwrap_or_default())?;
    let mut resp = agent.run(req).with_context(|| format!("{method} {url}"))?;
    let status = resp.status();
    let text = resp.body_mut().read_to_string()?;
    if !status.is_success() {
        // The arr apps echo submitted values, API keys too, in their error replies.
        let text: String = text.replace(key, "<key>").chars().take(300).collect();
        bail!("{method} {url}: HTTP {status}: {text}");
    }
    Ok(if text.is_empty() { Value::Null } else { serde_json::from_str(&text)? })
}

/// The schema entry for `implementation`, with these fields set: what the arr apps and Prowlarr
/// expect as the body of a new download client or application.
fn from_schema(schema: &Value, implementation: &str, fields: &[(&str, Value)]) -> Result<Value> {
    let mut item = (schema.as_array().into_iter().flatten())
        .find(|s| s["implementation"] == implementation)
        .cloned()
        .ok_or_else(|| anyhow!("no {implementation} in the schema"))?;
    for f in item["fields"].as_array_mut().into_iter().flatten() {
        if let Some((_, v)) = fields.iter().find(|(name, _)| f["name"] == *name) {
            f["value"] = v.clone();
        }
    }
    item["name"] = json!(implementation.replace("QBittorrent", "qBittorrent"));
    item["enable"] = json!(true);
    Ok(item)
}

fn find<'a>(list: &'a Value, implementation: &str) -> Option<&'a Value> {
    (list.as_array().into_iter().flatten()).find(|i| i["implementation"] == implementation)
}

/// True when every setting in `wanted` already has that value in `current`.
fn already(current: &Value, wanted: &Value) -> bool {
    (wanted.as_object().into_iter().flatten()).all(|(k, v)| current.get(k) == Some(v))
}

/// The one-time password that a new qBittorrent prints in its log, the newest one.
fn temporary_password(log: &str) -> Option<&str> {
    log.lines()
        .filter_map(|l| l.split("temporary password is provided for this session:").nth(1))
        .map(str::trim)
        .rfind(|p| !p.is_empty())
}

fn qbittorrent(agent: &ureq::Agent, base: &str) -> Result<()> {
    let prefs_url = format!("{base}/api/v2/app/preferences");
    // A new qBittorrent checks that the Host header names its own port, which the gate and Docker
    // do not keep (8081, not 8080), and answers 401 otherwise.
    let host = format!("127.0.0.1:{}", QBIT_INSIDE.1);
    let get_prefs = |cookie: Option<&str>| -> Result<ureq::http::Response<ureq::Body>> {
        let mut req = agent.get(&prefs_url).header("Host", &host);
        if let Some(c) = cookie {
            req = req.header("Cookie", c);
        }
        (req.call()).with_context(|| format!("qBittorrent at {base} does not answer; if it is still starting, run this again in a minute"))
    };
    let mut resp = get_prefs(None)?;
    let cookie = match resp.status().as_u16() {
        200 => None, // logins from this machine are already off
        401 | 403 => {
            let cookie = login(agent, base, &host)?;
            resp = get_prefs(Some(&cookie))?;
            ensure!(resp.status().is_success(), "qBittorrent at {base}: HTTP {} after login", resp.status());
            Some(cookie)
        }
        status => bail!("qBittorrent at {base}: HTTP {status}; if it is still starting, run this again in a minute"),
    };
    let current: Value = (resp.body_mut().read_json()).with_context(|| format!("qBittorrent at {base}"))?;
    let wanted: Value = serde_json::from_str(QBIT_PREFS)?;
    if already(&current, &wanted) {
        println!("qBittorrent: settings already set");
        return Ok(());
    }
    let mut set = agent.post(format!("{base}/api/v2/app/setPreferences")).header("Host", &host);
    if let Some(c) = &cookie {
        set = set.header("Cookie", c);
    }
    let resp = (set.send_form([("json", QBIT_PREFS)])).with_context(|| format!("qBittorrent at {base}"))?;
    if !resp.status().is_success() {
        bail!("qBittorrent setPreferences: HTTP {}", resp.status());
    }
    println!("qBittorrent: no login from this machine, downloads in /data/downloads/qbittorrent");
    Ok(())
}

/// Logs in as admin and gives the session cookie. The password comes from QBITTORRENT_PASSWORD,
/// else from the one-time password that a new qBittorrent prints in its log.
fn login(agent: &ureq::Agent, base: &str, host: &str) -> Result<String> {
    let password = match std::env::var("QBITTORRENT_PASSWORD") {
        Ok(p) if !p.is_empty() => p,
        _ => {
            let out = std::process::Command::new("docker")
                .args(["compose", "logs", "--no-log-prefix", "qbittorrent"])
                .current_dir(repo_dir())
                .output()
                .context("docker compose logs qbittorrent")?;
            ensure!(out.status.success(), "docker compose logs qbittorrent failed");
            temporary_password(&String::from_utf8_lossy(&out.stdout))
                .map(String::from)
                .context("qBittorrent asks for a login and its log has no one-time password: \
                          set QBITTORRENT_PASSWORD to its admin password")?
        }
    };
    let resp = agent
        .post(format!("{base}/api/v2/auth/login"))
        .header("Host", host)
        .send_form([("username", "admin"), ("password", password.as_str())])
        .with_context(|| format!("qBittorrent login at {base}"))?;
    (resp.headers().get_all("set-cookie").iter())
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| c.starts_with("SID=").then(|| c.split(';').next().unwrap_or("").to_string()))
        .context("qBittorrent refused the login: set QBITTORRENT_PASSWORD to its admin password \
                  (5 failed tries ban this machine for an hour)")
}

fn arr(agent: &ureq::Agent, a: &Arr, data: &std::path::Path) -> Result<String> {
    let key = arr_key(data, a.app)?;
    let api = format!("{}/api/v3", a.url);
    let clients = send(agent, "GET", &format!("{api}/downloadclient"), &key, None)?;
    if let Some(c) = find(&clients, "QBittorrent") {
        // The app's own Test button: it uses the saved settings, so it finds a wrong address too.
        let ok = c["enable"] == true
            && send(agent, "POST", &format!("{api}/downloadclient/test"), &key, Some(c)).is_ok();
        println!("{}: qBittorrent already set{}", a.app, if ok { "" } else {
            "; warning: it is turned off or its test fails, check it under Settings > Download Clients" });
    } else {
        let schema = send(agent, "GET", &format!("{api}/downloadclient/schema"), &key, None)?;
        let body = from_schema(&schema, "QBittorrent", &[
            ("host", json!(QBIT_INSIDE.0)),
            ("port", json!(QBIT_INSIDE.1)),
            (a.category.0, json!(a.category.1)),
        ])?;
        send(agent, "POST", &format!("{api}/downloadclient"), &key, Some(&body))?;
        println!("{}: added qBittorrent", a.app);
    }
    let roots = send(agent, "GET", &format!("{api}/rootfolder"), &key, None)?;
    if (roots.as_array().into_iter().flatten()).any(|r| r["path"] == a.root) {
        println!("{}: library folder {} already set", a.app, a.root);
    } else {
        send(agent, "POST", &format!("{api}/rootfolder"), &key, Some(&json!({"path": a.root})))?;
        println!("{}: library folder {}", a.app, a.root);
    }
    Ok(key)
}

fn prowlarr(agent: &ureq::Agent, base: &str, data: &std::path::Path, apps: &[(&Arr, String)]) -> Result<()> {
    let key = arr_key(data, "prowlarr")?;
    let api = format!("{base}/api/v1");
    let existing = send(agent, "GET", &format!("{api}/applications"), &key, None)?;
    let schema = send(agent, "GET", &format!("{api}/applications/schema"), &key, None)?;
    for (a, app_key) in apps {
        let implementation = if a.app == "sonarr" { "Sonarr" } else { "Radarr" };
        if let Some(app) = find(&existing, implementation) {
            // Prowlarr shows API keys as ********, so its own test checks the saved key instead.
            let ok = app["syncLevel"] != "disabled"
                && send(agent, "POST", &format!("{api}/applications/test"), &key, Some(app)).is_ok();
            println!("prowlarr: {implementation} already linked{}", if ok { "" } else {
                "; warning: its test fails (address or API key), check it under Settings > Apps" });
            continue;
        }
        let mut body = from_schema(&schema, implementation, &[
            ("prowlarrUrl", json!(PROWLARR_INSIDE)),
            ("baseUrl", json!(a.inside)),
            ("apiKey", json!(app_key)),
        ])?;
        body["syncLevel"] = json!("fullSync");
        // The body holds the app's own key, which Prowlarr may echo in an error.
        send(agent, "POST", &format!("{api}/applications"), &key, Some(&body))
            .map_err(|e| anyhow!(e.to_string().replace(app_key.as_str(), "<key>")))?;
        println!("prowlarr: linked {implementation}");
    }
    println!("prowlarr: add the indexers you want under Indexers; they reach Sonarr and Radarr by themselves");
    Ok(())
}

pub fn run(args: Args) -> Result<()> {
    let agent = agent();
    let data = data_dir();
    qbittorrent(&agent, &args.qbittorrent)?;
    let sonarr = Arr {
        app: "sonarr",
        url: args.sonarr,
        inside: "http://localhost:8989/sonarr",
        category: ("tvCategory", "tv-sonarr"),
        root: "/data/video/shows",
    };
    let radarr = Arr {
        app: "radarr",
        url: args.radarr,
        inside: "http://localhost:7878/radarr",
        category: ("movieCategory", "radarr"),
        root: "/data/video/movies",
    };
    let keys = [(&sonarr, arr(&agent, &sonarr, &data)?), (&radarr, arr(&agent, &radarr, &data)?)];
    prowlarr(&agent, &args.prowlarr, &data, &keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_entry_gets_the_fields() {
        let schema = json!([
            {"implementation": "Transmission", "fields": [{"name": "host", "value": "x"}]},
            {"implementation": "QBittorrent", "fields": [
                {"name": "host", "value": "localhost"}, {"name": "port", "value": 8080},
                {"name": "tvCategory", "value": "tv"}, {"name": "useSsl", "value": false}]}]);
        let item = from_schema(&schema, "QBittorrent", &[("host", json!("localhost")),
            ("tvCategory", json!("tv-sonarr"))]).unwrap();
        assert_eq!(item["name"], "qBittorrent");
        assert_eq!(item["enable"], true);
        assert_eq!(item["fields"][2]["value"], "tv-sonarr");
        assert_eq!(item["fields"][3]["value"], false);
        assert!(from_schema(&schema, "Deluge", &[]).is_err());
    }

    #[test]
    fn compares_settings() {
        let wanted = json!({"bypass_local_auth": true, "save_path": "/data/x"});
        assert!(already(&json!({"bypass_local_auth": true, "save_path": "/data/x", "other": 1}), &wanted));
        assert!(!already(&json!({"bypass_local_auth": false, "save_path": "/data/x"}), &wanted));
        assert!(!already(&json!({}), &wanted));
    }

    #[test]
    fn finds_the_newest_temporary_password() {
        let log = "WebUI will be started\nA temporary password is provided for this session: abc123\n\
                   restarted\nA temporary password is provided for this session: xyz789\n";
        assert_eq!(temporary_password(log), Some("xyz789"));
        assert_eq!(temporary_password("no password here"), None);
    }
}
