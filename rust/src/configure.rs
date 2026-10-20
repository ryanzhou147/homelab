//! Configures Lidarr for the music fetchers and checks that each piece is up.
//!
//! 1. Waits for Lidarr, and checks that it uses the /lidarr base and trusts Caddy's login.
//! 2. Adds /data/music (the Navidrome library) as Lidarr's root folder, where a new artist wants
//!    no albums until you pick them.
//! 3. Makes Lidarr refuse imports that would leave less than MIN_FREE_GB on the disk, and file
//!    each album in its own folder.
//! 4. Checks that slskd is logged in to Soulseek and that Soularr reaches both apps.
//! 5. Checks that the fetch gate refuses requests that web pages could send.
//!
//! Safe to run again: it changes only what differs.

use std::io::Read;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::common::{data_dir, repo_dir};

const LIDARR: &str = "http://127.0.0.1:8686/lidarr/api/v1";
const SLSKD: &str = "http://127.0.0.1:5030/slskd/api/v0";
const ROOT_FOLDER: &str = "/data/music";
const MIN_FREE_GB: u64 = 20;
const START_TIMEOUT_S: u64 = 180;

#[derive(clap::Args)]
pub struct Args {}

/// A new artist adds no wanted albums by itself, so one click cannot queue a whole discography.
fn monitor_only_picked() -> Value {
    json!({"defaultMonitorOption": "none", "defaultNewItemMonitorOption": "none"})
}

pub fn run(_args: Args) -> Result<()> {
    let key = api_key()?;
    let status = wait_for_lidarr(&key)?;
    let mut ok = status["urlBase"] == "/lidarr" && status["authentication"] == "external";
    println!(
        "{} lidarr {}: base {}, login {}",
        if ok { "ok  " } else { "FAIL" },
        py_str(&status["version"]),
        py_repr(&status["urlBase"]),
        py_repr(&status["authentication"]),
    );
    ensure_root_folder(&key)?;
    let wanted = json!({"minimumFreeSpaceWhenImporting": MIN_FREE_GB * 1024});
    let message = format!("Lidarr now refuses imports below {MIN_FREE_GB} GB free.");
    ensure_config(&key, "mediamanagement", &wanted, &message)?;
    // The album folder is part of the track name format, so without renaming every track of an
    // artist lands in one folder, and two albums' "01 Intro.mp3" collide.
    let message = "Lidarr now files each album in its own folder.";
    ensure_config(&key, "naming", &json!({"renameTracks": true}), message)?;
    ok = check_slskd() && ok;
    ok = check_soularr()? && ok;
    ok = check_gate()? && ok;
    std::process::exit(if ok { 0 } else { 1 });
}

/// Like Python's sys.exit("message").
fn die(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn api_key() -> Result<String> {
    let path = data_dir().join("secrets/lidarr.env");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let (_, key) = text
        .trim()
        .split_once('=')
        .context("lidarr.env has no KEY=value")?;
    Ok(key.to_string())
}

fn agent(timeout_s: u64, status_as_error: bool) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_s)))
        .http_status_as_error(status_as_error)
        .build()
        .into()
}

/// One JSON API call; a PUT or POST sends `body`. An empty reply gives null.
fn call(url: &str, key: &str, method: &str, body: Option<&Value>) -> Result<Value, ureq::Error> {
    let agent = agent(15, true);
    let mut reply = match (method, body) {
        ("GET", _) => agent.get(url).header("X-Api-Key", key).call()?,
        ("PUT", Some(b)) => agent.put(url).header("X-Api-Key", key).send_json(b)?,
        ("POST", Some(b)) => agent.post(url).header("X-Api-Key", key).send_json(b)?,
        _ => unreachable!("call({method}) without a body"),
    };
    let mut text = String::new();
    reply.body_mut().as_reader().read_to_string(&mut text)?;
    Ok(if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text)?
    })
}

fn wait_for_lidarr(key: &str) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(START_TIMEOUT_S);
    loop {
        match call(&format!("{LIDARR}/system/status"), key, "GET", None) {
            Ok(status) => return Ok(status),
            // A 401 comes from Lidarr itself, so waiting does not help. The gate gives 502 while
            // Lidarr starts.
            Err(ureq::Error::StatusCode(401)) => die(&format!(
                "Lidarr refused the API key in {}.",
                data_dir().join("secrets/lidarr.env").display()
            )),
            Err(ureq::Error::Json(err)) => return Err(err.into()),
            Err(err) if Instant::now() > deadline => die(&format!(
                "Lidarr did not answer within {START_TIMEOUT_S} s: {err}"
            )),
            Err(_) => std::thread::sleep(Duration::from_secs(3)),
        }
    }
}

/// True when a key of `wanted` has another value in `obj`. A missing key is an error, as in the
/// Python script.
fn differs(obj: &Value, wanted: &Value) -> Result<bool> {
    let mut any = false;
    for (k, v) in wanted.as_object().context("wanted is not an object")? {
        any |= obj.get(k).with_context(|| format!("Lidarr gave no {k}"))? != v;
    }
    Ok(any)
}

/// `{**obj, **wanted}`.
fn merged(obj: &Value, wanted: &Value) -> Value {
    let mut out = obj.clone();
    if let (Some(o), Some(w)) = (out.as_object_mut(), wanted.as_object()) {
        o.extend(w.clone());
    }
    out
}

fn ensure_root_folder(key: &str) -> Result<()> {
    let folders = call(&format!("{LIDARR}/rootfolder"), key, "GET", None)?;
    let existing = folders.as_array().into_iter().flatten().find(|f| {
        f["path"]
            .as_str()
            .is_some_and(|p| p.trim_end_matches('/') == ROOT_FOLDER)
    });
    let picked = monitor_only_picked();
    if let Some(existing) = existing {
        if differs(existing, &picked)? {
            let url = format!("{LIDARR}/rootfolder/{}", existing["id"]);
            call(&url, key, "PUT", Some(&merged(existing, &picked)))?;
            println!("Root folder now wants only the albums you pick.");
        }
        return Ok(());
    }
    let quality = call(&format!("{LIDARR}/qualityprofile"), key, "GET", None)?;
    let metadata = call(&format!("{LIDARR}/metadataprofile"), key, "GET", None)?;
    let first_id = |v: &Value| {
        v.as_array()
            .and_then(|a| a.first())
            .map(|p| p["id"].clone())
    };
    let (Some(quality_id), Some(metadata_id)) = (first_id(&quality), first_id(&metadata)) else {
        die("Lidarr has no quality or metadata profile: add one in its settings, then rerun.");
    };
    let body = merged(
        &json!({
            "name": "Music",
            "path": ROOT_FOLDER,
            "defaultQualityProfileId": quality_id,
            "defaultMetadataProfileId": metadata_id,
        }),
        &merged(&picked, &json!({"defaultTags": []})),
    );
    call(&format!("{LIDARR}/rootfolder"), key, "POST", Some(&body))?;
    println!("Added root folder {ROOT_FOLDER}.");
    Ok(())
}

fn ensure_config(key: &str, section: &str, wanted: &Value, message: &str) -> Result<()> {
    let config = call(&format!("{LIDARR}/config/{section}"), key, "GET", None)?;
    if !differs(&config, wanted)? {
        return Ok(());
    }
    let url = format!("{LIDARR}/config/{section}/{}", config["id"]);
    call(&url, key, "PUT", Some(&merged(&config, wanted)))?;
    println!("{message}");
    Ok(())
}

fn check_slskd() -> bool {
    let server = match call(&format!("{SLSKD}/server"), "", "GET", None) {
        Ok(server) => server,
        Err(err) => {
            println!("FAIL slskd: {err}");
            return false;
        }
    };
    let ok = server["isLoggedIn"].as_bool().unwrap_or(false);
    let mark = if ok { "ok  " } else { "FAIL" };
    println!("{mark} slskd: Soulseek state {}", py_str(&server["state"]));
    ok
}

/// `docker <args>` with stdout and stderr in one stream, trimmed; a non-zero exit is an error.
fn docker(args: &[&str]) -> Result<String> {
    // Tracebacks go to the container's stderr, so read both streams.
    let (mut reader, writer) = std::io::pipe()?;
    let mut child = {
        let mut cmd = Command::new("docker");
        cmd.args(args)
            .current_dir(repo_dir())
            .stdout(writer.try_clone()?)
            .stderr(writer);
        cmd.spawn()?
    };
    let mut out = String::new();
    reader.read_to_string(&mut out)?;
    ensure!(child.wait()?.success(), "docker {} failed", args.join(" "));
    Ok(out.trim().to_string())
}

fn check_soularr() -> Result<bool> {
    // Only the current run: a restarted container keeps the log of its earlier runs.
    let container = docker(&["compose", "ps", "-q", "soularr"])?;
    let started = docker(&["inspect", "-f", "{{.State.StartedAt}}", &container])?;
    let logs = docker(&["logs", "--since", &started, &container])?;
    let failed: Vec<&str> = logs
        .lines()
        .filter(|l| l.contains("ERROR") || l.contains("Traceback"))
        .collect();
    for line in &failed[failed.len().saturating_sub(3)..] {
        let chars: Vec<char> = line.chars().collect();
        let tail: String = chars[chars.len().saturating_sub(200)..].iter().collect();
        println!("FAIL soularr: {tail}");
    }
    if failed.is_empty() {
        println!("ok   soularr: no errors since it started");
    }
    Ok(failed.is_empty())
}

/// HOST from the environment, else from the repo's .env, so the command also runs alone.
fn lab_host() -> Result<String> {
    if let Some(host) = std::env::var("HOST").ok().filter(|h| !h.is_empty()) {
        return Ok(host);
    }
    let env = repo_dir().join(".env");
    let text =
        std::fs::read_to_string(&env).with_context(|| format!("cannot read {}", env.display()))?;
    match text.lines().find_map(|l| l.strip_prefix("HOST=")) {
        Some(host) => Ok(host.trim().to_string()),
        None => die(&format!(
            "No HOST in the environment or in {}.",
            env.display()
        )),
    }
}

/// The gate must refuse what a web page in the browser could send, and pass the lab.
fn check_gate() -> Result<bool> {
    let host = lab_host()?;
    let cases: [(Vec<(&str, String)>, u16); 5] = [
        (vec![("Host", "evil.example".into())], 403),
        (vec![("Origin", "https://evil.example".into())], 403),
        (vec![("Sec-Fetch-Site", "cross-site".into())], 403),
        (
            vec![
                ("Host", host.clone()),
                ("Origin", format!("https://{host}")),
                ("Sec-Fetch-Site", "same-origin".into()),
            ],
            200,
        ),
        (vec![], 200),
    ];
    let agent = agent(10, false);
    let mut ok = true;
    for (headers, want) in cases {
        let mut req = agent.get(format!("{SLSKD}/server"));
        for (k, v) in &headers {
            req = req.header(*k, v);
        }
        let got = req.call()?.status().as_u16();
        if got != want {
            println!("FAIL gate: {} gave {got}, want {want}", py_dict(&headers));
            ok = false;
        }
    }
    if ok {
        println!("ok   gate: refuses other sites, passes the lab and localhost");
    }
    Ok(ok)
}

/// Python's str() of a JSON value, as f-strings print it.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Python's repr() of a JSON value; strings get single quotes.
fn py_repr(v: &Value) -> String {
    match v {
        Value::String(s) => format!("'{s}'"),
        other => py_str(other),
    }
}

/// Python's repr() of a dict of strings.
fn py_dict(pairs: &[(&str, String)]) -> String {
    let items: Vec<String> = pairs.iter().map(|(k, v)| format!("'{k}': '{v}'")).collect();
    format!("{{{}}}", items.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn differs_only_on_another_value() {
        let config = json!({"id": 1, "renameTracks": true, "x": 2});
        assert!(!differs(&config, &json!({"renameTracks": true})).unwrap());
        assert!(differs(&config, &json!({"renameTracks": false})).unwrap());
        assert!(differs(&config, &json!({"missing": 1})).is_err());
        let space = json!({"minimumFreeSpaceWhenImporting": 20480});
        assert!(
            !differs(
                &space,
                &json!({"minimumFreeSpaceWhenImporting": MIN_FREE_GB * 1024})
            )
            .unwrap()
        );
    }

    #[test]
    fn merged_keeps_other_keys_and_overrides() {
        let out = merged(
            &json!({"id": 3, "defaultMonitorOption": "all"}),
            &monitor_only_picked(),
        );
        assert_eq!(
            out,
            json!({"id": 3, "defaultMonitorOption": "none", "defaultNewItemMonitorOption": "none"})
        );
    }

    #[test]
    fn prints_like_python() {
        assert_eq!(py_repr(&json!("/lidarr")), "'/lidarr'");
        assert_eq!(py_repr(&Value::Null), "None");
        assert_eq!(py_str(&json!("Connected, LoggedIn")), "Connected, LoggedIn");
        assert_eq!(py_dict(&[]), "{}");
        assert_eq!(
            py_dict(&[("Host", "a".into()), ("Origin", "https://a".into())]),
            "{'Host': 'a', 'Origin': 'https://a'}"
        );
    }
}
