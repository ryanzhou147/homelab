//! Writes ${DATA}/secrets/homepage.env: the API keys that the home page widgets use.
//!
//! The keys come from each app's own settings, and Lidarr's and Bazarr's from their secret files.
//! Jellyfin gets its own "Homepage" key, made with the key that Seerr already has. Navidrome gets
//! a Subsonic token and salt made from its password.
//!
//!     homelab homepage-secrets     (then: docker compose up -d homepage)
//!
//! Safe to run again: it uses the Jellyfin key that exists, and writes the file again.

use crate::common::data_dir;
use anyhow::{Context, anyhow, bail};
use md5::Digest;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

const JELLYFIN: &str = "http://127.0.0.1:8096/video";
const APP_NAME: &str = "Homepage";

#[derive(clap::Args)]
pub struct Args {}

pub(crate) fn arr_key(data: &Path, app: &str) -> anyhow::Result<String> {
    let path = data.join(app).join("config.xml");
    let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
    let re = regex::Regex::new(r"<ApiKey>(\w+)</ApiKey>")?;
    match re.captures(&text) {
        Some(c) => Ok(c[1].to_string()),
        None => bail!("No ApiKey in {}", path.display()),
    }
}

fn jellyfin_key(admin_key: &str) -> anyhow::Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let auth = format!("MediaBrowser Token=\"{admin_key}\"");
    let find = || -> anyhow::Result<Option<String>> {
        let keys: serde_json::Value = agent
            .get(format!("{JELLYFIN}/Auth/Keys"))
            .header("Authorization", &auth)
            .call()?
            .body_mut()
            .read_json()?;
        let items = keys["Items"]
            .as_array()
            .ok_or(anyhow!("no Items in /Auth/Keys"))?;
        Ok(items
            .iter()
            .find(|k| k["AppName"] == APP_NAME)
            .and_then(|k| k["AccessToken"].as_str())
            .filter(|t| !t.is_empty())
            .map(String::from))
    };
    if find()?.is_none() {
        agent
            .post(format!("{JELLYFIN}/Auth/Keys?app={APP_NAME}"))
            .header("Authorization", &auth)
            .send_empty()?;
    }
    find()?.ok_or(anyhow!("Jellyfin did not make the Homepage API key"))
}

/// The value of NAME= in an env file, without its quotes.
fn env_value(path: &Path, name: &str) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let re = regex::Regex::new(&format!("(?m)^{}=(.*)$", regex::escape(name)))?;
    match re.captures(&text) {
        Some(c) => Ok(c[1].trim().trim_matches(['\'', '"']).to_string()),
        None => bail!("No {name} in {}", path.display()),
    }
}

/// A one-line secret file, such as the Bazarr key from setup-bazarr.sh.
fn file_value(path: &Path) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    Ok(text.trim().to_string())
}

/// The Subsonic token: md5 of the password and the salt, in hex.
fn subsonic_token(password: &str, salt: &str) -> String {
    hex::encode(md5::Md5::digest(format!("{password}{salt}")))
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    let data = data_dir();
    let secrets = data.join("secrets");
    let seerr_path = data.join("seerr").join("settings.json");
    let seerr: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&seerr_path).with_context(|| seerr_path.display().to_string())?,
    )?;
    let seerr_str = |section: &str| {
        seerr[section]["apiKey"]
            .as_str()
            .map(String::from)
            .ok_or(anyhow!("No {section}.apiKey in {}", seerr_path.display()))
    };
    let mut raw = [0u8; 8];
    getrandom::fill(&mut raw).map_err(|e| anyhow!("random salt: {e}"))?;
    let salt = hex::encode(raw);
    let password = env_value(&secrets.join("navidrome.env"), "NAVIDROME_PASSWORD")?;
    let token = subsonic_token(&password, &salt);
    let mut values = vec![
        ("SEERR_KEY", seerr_str("main")?),
        ("JELLYFIN_KEY", jellyfin_key(&seerr_str("jellyfin")?)?),
        ("NAVIDROME_SALT", salt),
        ("NAVIDROME_TOKEN", token),
    ];
    // The fetchers are optional: a missing one leaves its card without numbers.
    let optional = [
        ("SONARR_KEY", arr_key(&data, "sonarr")),
        ("RADARR_KEY", arr_key(&data, "radarr")),
        (
            "LIDARR_KEY",
            env_value(&secrets.join("lidarr.env"), "LIDARR__AUTH__APIKEY"),
        ),
        ("PROWLARR_KEY", arr_key(&data, "prowlarr")),
        ("BAZARR_KEY", file_value(&secrets.join("bazarr_apikey"))),
    ];
    for (k, v) in optional {
        match v {
            Ok(v) => values.push((k, v)),
            Err(err) => eprintln!("Skipped {k}: {err}"),
        }
    }
    let out = secrets.join("homepage.env");
    let tmp = out.with_extension("env.tmp");
    let _ = std::fs::remove_file(&tmp);
    // A new file with mode 600, so the keys are never readable by others, then an atomic rename.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    for (k, v) in &values {
        writeln!(file, "HOMEPAGE_VAR_{k}={v}")?;
    }
    file.sync_all()?;
    std::fs::rename(&tmp, &out)?;
    println!("Wrote {} values to {}", values.len(), out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_value_strips_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.env");
        std::fs::write(&path, "A=1\nNAVIDROME_PASSWORD= 'p w' \n").unwrap();
        assert_eq!(env_value(&path, "NAVIDROME_PASSWORD").unwrap(), "p w");
        assert!(env_value(&path, "B").is_err());
    }

    #[test]
    fn token_is_md5_of_password_and_salt() {
        // md5("sesame" + "c19b2d")
        assert_eq!(
            subsonic_token("sesame", "c19b2d"),
            "26719a1196d2a940705a59634eb18eab"
        );
    }
}
