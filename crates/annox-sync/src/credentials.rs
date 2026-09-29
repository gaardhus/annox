//! Per-user hub credentials (§7.2), never stored in `.annox/`.
//!
//! The file is `$ANNOX_CREDENTIALS_FILE`, or `$XDG_CONFIG_HOME/annox/credentials.json`
//! (default `~/.config/annox/credentials.json`):
//!
//! ```json
//! { "hubs": { "wss://hub.example.org/w/3f9c": { "token": "…" } } }
//! ```
//!
//! A hub is used only if it has an entry. The entry may omit `token` for a
//! hub that needs none.

use std::path::PathBuf;

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credential {
    pub token: Option<String>,
}

pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ANNOX_CREDENTIALS_FILE") {
        return Some(PathBuf::from(p));
    }
    Some(annox_core::config::dir()?.join("credentials.json"))
}

/// The credential for hub `url`, if configured.
pub fn lookup(url: &str) -> Option<Credential> {
    let text = std::fs::read_to_string(path()?).ok()?;
    let file: Value = serde_json::from_str(&text).ok()?;
    let entry = file["hubs"].get(url)?;
    Some(Credential { token: entry["token"].as_str().map(str::to_owned) })
}
