//! Prometheus exporter for the home lab apps, on 127.0.0.1:9102/metrics.
//!
//! Each scrape asks the apps (through the PC's 127.0.0.1 ports, as the fetch gate requires) for
//! health, download speeds, queue and library sizes, and reads the spotdl progress file and the CPU
//! temperature. The API keys come from homepage.env (homelab homepage-secrets). The Grafana
//! graphs on the home page are drawn from these numbers. Runs as homelab-lab-metrics.service.

use crate::common::{data_dir, lab_user, load_env, setting};
use anyhow::{Context, anyhow};
use serde_json::{Number, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

// All checks run at the same time, and *arr queue + wanted is 2 x 3 s: under the 10 s scrape limit.
const TIMEOUT: Duration = Duration::from_secs(3);
const HWMON: &str = "/sys/class/hwmon";
const ARR: [(&str, &str, &str); 3] = [
    // app, API root, key variable
    (
        "sonarr",
        "http://127.0.0.1:8989/sonarr/api/v3",
        "HOMEPAGE_VAR_SONARR_KEY",
    ),
    (
        "radarr",
        "http://127.0.0.1:7878/radarr/api/v3",
        "HOMEPAGE_VAR_RADARR_KEY",
    ),
    (
        "lidarr",
        "http://127.0.0.1:8686/lidarr/api/v1",
        "HOMEPAGE_VAR_LIDARR_KEY",
    ),
];

#[derive(clap::Args)]
pub struct Args {
    /// The address to serve the metrics on.
    #[arg(long, default_value = "127.0.0.1:9102")]
    listen: String,
}

type Sample = (&'static str, Vec<(&'static str, String)>, Number);
/// GET a URL with extra headers and give the body; a fake in the tests.
type Get<'a> = &'a (dyn Fn(&str, &[(&str, &str)]) -> anyhow::Result<Vec<u8>> + Sync);
type Job = fn(&Lab, Get, &str, &mut Vec<Sample>) -> anyhow::Result<()>;

struct Lab {
    host: String,
    data: PathBuf,
    env: HashMap<String, String>,
    health: Vec<(&'static str, String)>,
}

impl Lab {
    fn from_env() -> anyhow::Result<Self> {
        let host = setting("HOST").context("Set HOST in .env")?;
        let health = vec![
            ("jellyfin", "http://127.0.0.1:8096/video/health".into()),
            ("seerr", "http://127.0.0.1:5055/api/v1/status".into()),
            ("navidrome", format!("https://{host}/music/rest/ping.view")),
            ("lidarr", "http://127.0.0.1:8686/lidarr/ping".into()),
            ("slskd", "http://127.0.0.1:5030/slskd/health".into()),
            (
                "qbittorrent",
                "http://127.0.0.1:8081/api/v2/app/version".into(),
            ),
            ("sonarr", "http://127.0.0.1:8989/sonarr/ping".into()),
            ("radarr", "http://127.0.0.1:7878/radarr/ping".into()),
            ("prowlarr", "http://127.0.0.1:9696/prowlarr/ping".into()),
            ("authelia", "http://127.0.0.1:9091/api/health".into()),
            ("homepage", "http://127.0.0.2:3010/api/healthcheck".into()),
        ];
        Ok(Lab {
            host,
            data: data_dir(),
            // The API keys from homepage-secrets; the environment wins.
            env: load_env(&data_dir().join("secrets/homepage.env"))
                .into_iter()
                .chain(std::env::vars())
                .collect(),
            health,
        })
    }

    fn var(&self, name: &str) -> anyhow::Result<&str> {
        self.env
            .get(name)
            .map(String::as_str)
            .ok_or(anyhow!("{name} is not set"))
    }
}

fn http_get(
    agent: &ureq::Agent,
    host: &str,
    url: &str,
    headers: &[(&str, &str)],
) -> anyhow::Result<Vec<u8>> {
    let mut req = agent.get(url);
    if url.contains("127.0.0.2") {
        // Homepage answers only to the lab's name.
        req = req.header("Host", host);
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    Ok(req.call()?.body_mut().read_to_vec()?)
}

fn json(body: &[u8]) -> anyhow::Result<Value> {
    Ok(serde_json::from_slice(body)?)
}

/// The number at a JSON path; an error names the path, as Python's KeyError does.
fn num(v: &Value, path: &[&str]) -> anyhow::Result<Number> {
    let found = path.iter().try_fold(v, |v, k| v.get(k));
    match found {
        Some(Value::Number(n)) => Ok(n.clone()),
        Some(Value::Bool(b)) => Ok(Number::from(u8::from(*b))),
        _ => Err(anyhow!("no number at {}", path.join("."))),
    }
}

fn qbittorrent(_: &Lab, get: Get, _: &str, out: &mut Vec<Sample>) -> anyhow::Result<()> {
    let info = json(&get("http://127.0.0.1:8081/api/v2/transfer/info", &[])?)?;
    let label = |d: &str| vec![("direction", d.to_string())];
    let metric = "homelab_qbittorrent_bytes_per_second";
    out.push((metric, label("down"), num(&info, &["dl_info_speed"])?));
    out.push((metric, label("up"), num(&info, &["up_info_speed"])?));
    Ok(())
}

fn arr(lab: &Lab, get: Get, app: &str, out: &mut Vec<Sample>) -> anyhow::Result<()> {
    let (_, root, key) = ARR.iter().find(|(a, ..)| *a == app).expect("app is in ARR");
    let auth = [("X-Api-Key", lab.var(key)?)];
    let queue = num(
        &json(&get(&format!("{root}/queue/status"), &auth)?)?,
        &["totalCount"],
    )?;
    let wanted = json(&get(&format!("{root}/wanted/missing?pageSize=1"), &auth)?)?;
    let wanted = num(&wanted, &["totalRecords"])?;
    out.push(("homelab_queue_items", vec![("app", app.to_string())], queue));
    out.push((
        "homelab_wanted_items",
        vec![("app", app.to_string())],
        wanted,
    ));
    Ok(())
}

fn library(lab: &Lab, get: Get, _: &str, out: &mut Vec<Sample>) -> anyhow::Result<()> {
    let token = format!(
        "MediaBrowser Token=\"{}\"",
        lab.var("HOMEPAGE_VAR_JELLYFIN_KEY")?
    );
    let url = "http://127.0.0.1:8096/video/Items/Counts";
    let counts = json(&get(url, &[("Authorization", &token)])?)?;
    let kind = |k: &str| vec![("kind", k.to_string())];
    out.push((
        "homelab_library_items",
        kind("movies"),
        num(&counts, &["MovieCount"])?,
    ));
    out.push((
        "homelab_library_items",
        kind("episodes"),
        num(&counts, &["EpisodeCount"])?,
    ));
    let (token, salt) = (
        lab.var("HOMEPAGE_VAR_NAVIDROME_TOKEN")?,
        lab.var("HOMEPAGE_VAR_NAVIDROME_SALT")?,
    );
    let url = format!(
        "https://{}/music/rest/getScanStatus?u={}&t={token}&s={salt}&v=1.16.1&c=lab-metrics&f=json",
        lab.host,
        lab_user()
    );
    let scan = json(&get(&url, &[])?)?;
    let songs = num(&scan, &["subsonic-response", "scanStatus", "count"])?;
    out.push(("homelab_library_items", kind("songs"), songs));
    Ok(())
}

fn spotdl(lab: &Lab, _: Get, _: &str, out: &mut Vec<Sample>) -> anyhow::Result<()> {
    let path = lab.data.join("status").join("downloads.json");
    let status = json(&std::fs::read(&path).with_context(|| path.display().to_string())?)?;
    for state in ["done", "failed", "left"] {
        out.push((
            "homelab_spotdl_songs",
            vec![("state", state.into())],
            num(&status, &[state])?,
        ));
    }
    out.push((
        "homelab_spotdl_running",
        vec![],
        num(&status, &["running"])?,
    ));
    Ok(())
}

/// Tctl of the Ryzen (k10temp), in degrees C.
fn cpu_temperature(hwmon: &Path) -> anyhow::Result<Option<f64>> {
    for chip in std::fs::read_dir(hwmon)? {
        let chip = chip?.path();
        let is_hwmon = chip
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("hwmon"));
        if is_hwmon && std::fs::read_to_string(chip.join("name"))?.trim() == "k10temp" {
            let milli: i64 = std::fs::read_to_string(chip.join("temp1_input"))?
                .trim()
                .parse()?;
            return Ok(Some(milli as f64 / 1000.0));
        }
    }
    Ok(None)
}

fn temperature(_: &Lab, _: Get, _: &str, out: &mut Vec<Sample>) -> anyhow::Result<()> {
    if let Some(n) = cpu_temperature(Path::new(HWMON))?.and_then(Number::from_f64) {
        out.push(("homelab_cpu_temperature_celsius", vec![], n));
    }
    Ok(())
}

fn collect(lab: &Lab, get: Get) -> Vec<Sample> {
    let jobs: [(&str, Job); 7] = [
        ("qbittorrent", qbittorrent),
        ("library", library),
        ("spotdl", spotdl),
        ("temperature", temperature),
        ("sonarr", arr),
        ("radarr", arr),
        ("lidarr", arr),
    ];
    // All in parallel, so one hung app cannot push the scrape past Prometheus's 10 s timeout.
    std::thread::scope(|s| {
        let health: Vec<_> = (lab.health.iter())
            .map(|(name, url)| (*name, s.spawn(move || get(url, &[]).is_ok())))
            .collect();
        let jobs: Vec<_> = (jobs.into_iter())
            .map(|(name, job)| {
                s.spawn(move || {
                    let mut out = Vec::new();
                    if let Err(err) = job(lab, get, name, &mut out) {
                        // One app down must not hide the others.
                        println!("lab-metrics: {name}: {err:#}");
                    }
                    out
                })
            })
            .collect();
        let mut samples: Vec<Sample> = health
            .into_iter()
            .map(|(name, h)| {
                let up = Number::from(u8::from(h.join().unwrap_or(false)));
                (
                    "homelab_service_up",
                    vec![("service", name.to_string())],
                    up,
                )
            })
            .collect();
        for job in jobs {
            samples.extend(job.join().unwrap_or_default());
        }
        samples
    })
}

fn render(samples: &[Sample]) -> String {
    samples
        .iter()
        .map(|(name, labels, value)| {
            let text: Vec<String> = labels.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect();
            if text.is_empty() {
                format!("{name} {value}\n")
            } else {
                format!("{name}{{{}}} {value}\n", text.join(","))
            }
        })
        .collect()
}

fn handle(req: tiny_http::Request, lab: &Lab, get: Get) -> std::io::Result<()> {
    if *req.method() != tiny_http::Method::Get {
        return req.respond(tiny_http::Response::empty(501));
    }
    if req.url() != "/metrics" {
        return req.respond(tiny_http::Response::empty(404));
    }
    let content_type = tiny_http::Header::from_bytes("Content-Type", "text/plain; version=0.0.4")
        .expect("valid header");
    req.respond(
        tiny_http::Response::from_string(render(&collect(lab, get))).with_header(content_type),
    )
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let lab = Lab::from_env()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let get = |url: &str, headers: &[(&str, &str)]| http_get(&agent, &lab.host, url, headers);
    let server = tiny_http::Server::http(&args.listen)
        .map_err(|e| anyhow!("cannot listen on {}: {e}", args.listen))?;
    std::thread::scope(|s| {
        for req in server.incoming_requests() {
            let (lab, get) = (&lab, &get);
            s.spawn(move || {
                if let Err(err) = handle(req, lab, get) {
                    println!("lab-metrics: reply: {err}");
                }
            });
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_writes_prometheus_text() {
        let samples = [
            ("a_up", vec![("service", "x".to_string())], Number::from(1)),
            ("b", vec![], Number::from_f64(2.5).unwrap()),
        ];
        assert_eq!(render(&samples), "a_up{service=\"x\"} 1\nb 2.5\n");
    }

    #[test]
    fn one_app_down_does_not_hide_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let lab = Lab {
            host: "lab.example".into(),
            data: dir.path().into(),
            env: ["SONARR", "RADARR", "LIDARR"]
                .map(|k| (format!("HOMEPAGE_VAR_{k}_KEY"), "k".to_string()))
                .into(),
            health: vec![
                ("sonarr", "http://sonarr/ping".into()),
                ("radarr", "http://radarr/ping".into()),
            ],
        };
        let fake = |url: &str, _: &[(&str, &str)]| -> anyhow::Result<Vec<u8>> {
            anyhow::ensure!(!url.contains("sonarr"), "down");
            Ok(br#"{"totalCount": 3, "totalRecords": 7}"#.to_vec())
        };
        let samples = collect(&lab, &fake);
        let apps: Vec<&str> = (samples.iter())
            .filter(|s| s.0 == "homelab_queue_items")
            .map(|s| s.1[0].1.as_str())
            .collect();
        assert_eq!(apps, ["radarr", "lidarr"]);
        let up: Vec<(&str, String)> = (samples.iter())
            .filter(|s| s.0 == "homelab_service_up")
            .map(|s| (s.1[0].1.as_str(), s.2.to_string()))
            .collect();
        assert_eq!(up, [("sonarr", "0".into()), ("radarr", "1".into())]);
    }

    #[test]
    fn homepage_gets_the_lab_name_as_host() {
        let server = tiny_http::Server::http("127.0.0.2:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let seen = std::thread::spawn(move || {
            let req = server.recv().unwrap();
            let host = (req.headers().iter())
                .find(|h| h.field.equiv("Host"))
                .map(|h| h.value.to_string());
            req.respond(tiny_http::Response::from_string("ok")).unwrap();
            host
        });
        let agent = ureq::Agent::new_with_defaults();
        let body = http_get(
            &agent,
            "lab.example",
            &format!("http://127.0.0.2:{port}/"),
            &[],
        );
        assert_eq!(body.unwrap(), b"ok");
        assert_eq!(seen.join().unwrap().as_deref(), Some("lab.example"));
    }

    #[test]
    fn temperature_is_read_from_k10temp() {
        let dir = tempfile::tempdir().unwrap();
        for (chip, name) in [("hwmon0", "nvme"), ("hwmon1", "k10temp")] {
            std::fs::create_dir(dir.path().join(chip)).unwrap();
            std::fs::write(dir.path().join(chip).join("name"), format!("{name}\n")).unwrap();
            std::fs::write(dir.path().join(chip).join("temp1_input"), "45000\n").unwrap();
        }
        let celsius = cpu_temperature(dir.path())
            .unwrap()
            .and_then(Number::from_f64);
        assert_eq!(celsius.unwrap().to_string(), "45.0");
    }
}
