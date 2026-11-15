//! The Playlists page at https://HOST/playlists: paste a public Spotify playlist link or pick an
//! Exportify CSV, and the add-playlist steps run in the background while the page shows their
//! progress. Caddy lets only lab admins in; this server listens on 127.0.0.1 only.
//!
//!     homelab playlist-web            (systemd: homelab-playlists.service)

use std::io::Read;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response};

use crate::playlist::{self, LINK_LIMIT, Playlist};

// Liked Songs as an Exportify CSV is about 0.5 KB a song; this allows about 60,000 songs.
const MAX_BODY: u64 = 32 << 20;
// shortcut: jobs live in memory and a restart forgets them, enough for one household; keep them
// on disk if the page should show history across restarts.
const KEEP_JOBS: usize = 20;

#[derive(clap::Args)]
pub struct Args {
    #[arg(long, default_value = "127.0.0.1:9103")]
    listen: String,
}

enum Source {
    Link(String),
    Csv { file_name: String, text: String },
}

#[derive(Clone, serde::Serialize)]
struct Job {
    id: usize,
    title: String,
    state: &'static str,
    lines: Vec<String>,
}

type Jobs = Arc<Mutex<Vec<Job>>>;

fn update(jobs: &Jobs, id: usize, f: impl FnOnce(&mut Job)) {
    if let Some(job) = jobs.lock().unwrap().iter_mut().find(|j| j.id == id) {
        f(job);
    }
}

/// Runs the queued jobs one at a time, so two playlists never search Soulseek at once.
fn worker(jobs: Jobs, queue: Receiver<(usize, Source)>) {
    for (id, source) in queue {
        update(&jobs, id, |j| j.state = "running");
        let mut log = |line: String| update(&jobs, id, |j| j.lines.push(line));
        let result = read(source).and_then(|p| {
            update(&jobs, id, |j| j.title = p.name.clone());
            playlist::add(&p, &mut log)
        });
        let (state, last) = match result {
            Ok(()) => ("done", None),
            Err(e) => ("failed", Some(format!("Stopped: {e:#}"))),
        };
        update(&jobs, id, |j| {
            j.state = state;
            j.lines.extend(last);
        });
    }
}

fn read(source: Source) -> Result<Playlist> {
    match source {
        Source::Link(link) => playlist::from_link(&link),
        Source::Csv { file_name, text } => playlist::from_csv(&file_name, &text),
    }
}

fn header(req: &Request, name: &'static str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

/// The names this page answers to: the lab's (through Caddy) and its own listen address. A
/// page on another site that points its own name at 127.0.0.1 (DNS rebinding) gets nothing.
fn allowed_hosts(listen: &str) -> Vec<String> {
    let port = listen.rsplit(':').next().unwrap_or_default();
    ["HOST", "OLD_HOST"]
        .iter()
        .filter_map(|k| crate::common::setting(k))
        .chain([listen.to_string(), format!("localhost:{port}")])
        .collect()
}

/// The request body as a Source, after the checks a web form from another site cannot pass.
fn parse_add(req: &mut Request) -> Result<Source> {
    if !header(req, "Content-Type").is_some_and(|v| v.starts_with("application/json")) {
        return Err(anyhow!("send JSON"));
    }
    let host = header(req, "Host").unwrap_or_default();
    if header(req, "Origin").is_some_and(|o| o != format!("https://{host}")) {
        return Err(anyhow!("wrong origin"));
    }
    let mut body = String::new();
    req.as_reader()
        .take(MAX_BODY + 1)
        .read_to_string(&mut body)?;
    if body.len() as u64 > MAX_BODY {
        return Err(anyhow!("File too large (max {} MB)", MAX_BODY >> 20));
    }
    let v: Value = serde_json::from_str(&body).context("not JSON")?;
    if let Some(link) = v["link"].as_str() {
        playlist::playlist_id(link)
            .context("Not a Spotify playlist link (open.spotify.com/playlist/...)")?;
        return Ok(Source::Link(link.trim().to_string()));
    }
    match (v["file_name"].as_str(), v["csv"].as_str()) {
        (Some(name), Some(text)) if !text.is_empty() => Ok(Source::Csv {
            file_name: name.to_string(),
            text: text.to_string(),
        }),
        _ => Err(anyhow!("Paste a link or pick a CSV file")),
    }
}

fn reply(req: Request, code: u16, content_type: &str, body: String) -> std::io::Result<()> {
    let header = Header::from_bytes("Content-Type", content_type).unwrap();
    req.respond(
        Response::from_string(body)
            .with_status_code(code)
            .with_header(header),
    )
}

pub fn run(args: Args) -> Result<()> {
    let server = tiny_http::Server::http(&args.listen)
        .map_err(|e| anyhow!("listen on {}: {e}", args.listen))?;
    let jobs: Jobs = Arc::default();
    let (tx, rx) = channel();
    let worker_jobs = jobs.clone();
    std::thread::spawn(move || worker(worker_jobs, rx));
    let hosts = allowed_hosts(&args.listen);
    let mut next_id = 1;
    println!("Playlists page on http://{}/playlists/", args.listen);
    for mut req in server.incoming_requests() {
        if !header(&req, "Host").is_some_and(|h| hosts.contains(&h)) {
            let _ = reply(req, 403, "text/plain", "Forbidden".into());
            continue;
        }
        let path = req.url().split('?').next().unwrap_or("").to_string();
        let result = match (req.method(), path.as_str()) {
            (Method::Get, "/playlists" | "/playlists/") => {
                let page = PAGE.replace("{LIMIT}", &LINK_LIMIT.to_string());
                reply(req, 200, "text/html; charset=utf-8", page)
            }
            (Method::Get, "/playlists/jobs") => {
                let list = jobs
                    .lock()
                    .unwrap()
                    .iter()
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>();
                reply(req, 200, "application/json", serde_json::to_string(&list)?)
            }
            (Method::Post, "/playlists/add") => match parse_add(&mut req) {
                Ok(source) => {
                    let title = match &source {
                        Source::Link(l) => l.clone(),
                        Source::Csv { file_name, .. } => file_name.clone(),
                    };
                    let id = next_id;
                    next_id += 1;
                    {
                        let mut list = jobs.lock().unwrap();
                        list.push(Job {
                            id,
                            title,
                            state: "queued",
                            lines: Vec::new(),
                        });
                        // Forget the oldest finished jobs; a queued or running one stays.
                        while list.len() > KEEP_JOBS {
                            match list
                                .iter()
                                .position(|j| matches!(j.state, "done" | "failed"))
                            {
                                Some(i) => drop(list.remove(i)),
                                None => break,
                            }
                        }
                    }
                    tx.send((id, source))?;
                    reply(req, 200, "application/json", json!({"id": id}).to_string())
                }
                Err(e) => {
                    let body = json!({"error": e.to_string()}).to_string();
                    reply(req, 400, "application/json", body)
                }
            },
            _ => reply(req, 404, "text/plain", "Not found".into()),
        };
        if let Err(e) = result {
            eprintln!("reply failed: {e}");
        }
    }
    Ok(())
}

const PAGE: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Playlists</title>
<style>
:root { --bg:#f6f7f9; --card:#fff; --text:#1d2129; --muted:#5f6670; --line:#dde1e6; --accent:#1db954; --bad:#d63b3b; }
@media (prefers-color-scheme: dark) { :root { --bg:#121417; --card:#1c1f24; --text:#e8eaed; --muted:#9aa1ab; --line:#2c3138; --bad:#ff6b6b; } }
* { box-sizing:border-box; }
body { margin:0; font:16px/1.5 system-ui, sans-serif; background:var(--bg); color:var(--text); }
main { max-width:760px; margin:0 auto; padding:32px 16px; }
h1 { font-size:24px; margin:0 0 4px; }
p.lead { color:var(--muted); margin:0 0 24px; }
.card { background:var(--card); border:1px solid var(--line); border-radius:12px; padding:20px; margin-bottom:16px; }
label { display:block; font-weight:600; margin-bottom:6px; }
.row { display:flex; gap:8px; flex-wrap:wrap; }
input[type=url] { flex:1; min-width:0; padding:10px 12px; border:1px solid var(--line); border-radius:8px; background:var(--bg); color:var(--text); font:inherit; }
button { padding:10px 18px; border:0; border-radius:8px; background:var(--accent); color:#04130a; font:inherit; font-weight:600; cursor:pointer; }
button:disabled { opacity:.5; cursor:default; }
.or { color:var(--muted); text-align:center; margin:14px 0; }
.hint { color:var(--muted); font-size:14px; margin-top:6px; }
#msg { min-height:1.5em; margin-top:10px; color:var(--bad); }
.job h2 { font-size:16px; margin:0 0 4px; overflow-wrap:anywhere; }
.state { font-size:13px; font-weight:600; text-transform:uppercase; letter-spacing:.04em; color:var(--muted); }
.state.running, .state.queued { color:#d9a400; } .state.done { color:var(--accent); } .state.failed { color:var(--bad); }
ol { margin:8px 0 0; padding-left:20px; font-size:14px; color:var(--muted); max-height:220px; overflow:auto; }
</style></head>
<body><main>
<h1>Add a Spotify playlist</h1>
<p class="lead">The songs you have go into a Navidrome playlist at once. Missing songs download from Soulseek and join it when they arrive.</p>
<div class="card">
  <label for="link">Public playlist link</label>
  <div class="row"><input id="link" type="url" placeholder="https://open.spotify.com/playlist/..." autocomplete="off"><button id="add-link">Add</button></div>
  <div class="hint">Works for any public playlist, Spotify's own too. Reads the first {LIMIT} songs.</div>
  <div class="or">or</div>
  <label for="file">Exportify CSV (for over {LIMIT} songs)</label>
  <div class="row"><input id="file" type="file" accept=".csv,text/csv"><button id="add-file">Add</button></div>
  <div class="hint">Export the playlist at exportify.app, then pick the file here.</div>
  <div id="msg" role="status"></div>
</div>
<div id="jobs"></div>
</main>
<script>
const msg = document.getElementById("msg");
async function send(body) {
  msg.textContent = "";
  const r = await fetch("/playlists/add", {method: "POST", headers: {"Content-Type": "application/json"}, body: JSON.stringify(body)});
  if (!r.ok) { msg.textContent = (await r.json().catch(() => ({}))).error || "Could not add it"; return false; }
  refresh(); return true;
}
document.getElementById("add-link").onclick = async () => {
  const input = document.getElementById("link");
  if (await send({link: input.value.trim()})) input.value = "";
};
document.getElementById("add-file").onclick = async () => {
  const input = document.getElementById("file"), file = input.files[0];
  if (!file) { msg.textContent = "Pick a CSV file first"; return; }
  if (await send({file_name: file.name, csv: await file.text()})) input.value = "";
};
function render(jobs) {
  const box = document.getElementById("jobs");
  box.replaceChildren(...jobs.map(j => {
    const card = document.createElement("div"); card.className = "card job";
    const h = document.createElement("h2"); h.textContent = j.title;
    const s = document.createElement("div"); s.className = "state " + j.state; s.textContent = j.state;
    const ol = document.createElement("ol");
    for (const line of j.lines) { const li = document.createElement("li"); li.textContent = line; ol.append(li); }
    card.append(h, s, ol); return card;
  }));
}
async function refresh() { const r = await fetch("/playlists/jobs"); if (r.ok) render(await r.json()); }
refresh(); setInterval(refresh, 3000);
</script></body></html>
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_http::TestRequest;

    fn post(content_type: &str, origin: Option<&str>, body: &'static str) -> Result<Source> {
        let mut req = TestRequest::new()
            .with_method(Method::Post)
            .with_path("/playlists/add")
            .with_header(Header::from_bytes("Host", "lab.example").unwrap())
            .with_header(Header::from_bytes("Content-Type", content_type).unwrap())
            .with_body(body);
        if let Some(o) = origin {
            req = req.with_header(Header::from_bytes("Origin", o).unwrap());
        }
        parse_add(&mut req.into())
    }

    #[test]
    fn refuses_requests_another_site_can_send() {
        let link = r#"{"link": "https://open.spotify.com/playlist/37i9dQZF1DX4UtSsGT1Sbe"}"#;
        assert!(post("application/json", Some("https://lab.example"), link).is_ok());
        assert!(post("application/json", None, link).is_ok());
        assert!(post("application/json", Some("https://evil.example"), link).is_err());
        assert!(post("application/json", Some("http://lab.example"), link).is_err());
        assert!(post("text/plain", Some("https://lab.example"), link).is_err());
        assert!(
            post(
                "application/json",
                None,
                r#"{"link": "https://evil.example/x"}"#
            )
            .is_err()
        );
        assert!(
            post(
                "application/json",
                None,
                r#"{"file_name": "a.csv", "csv": ""}"#
            )
            .is_err()
        );
    }
}
