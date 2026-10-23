//! `annox config`: the settings in effect, and where each comes from.

use std::path::{Path, PathBuf};

use annox_core::storage::Workspace;
use serde_json::{json, Value};

/// The settings `annox` uses in `cwd`, each with where it comes from, and the
/// files they're read from. Never fails on a broken file: it reports it, since
/// finding one is what this is for.
pub(super) fn config(cwd: &Path) -> anyhow::Result<Value> {
    let ws = Workspace::find(cwd);
    let config = annox_core::config::load();
    let env = |key| std::env::var(key).ok().filter(|v| !v.is_empty());

    // The value is what write commands use; the sources follow the same
    // precedence as `resolve_author`.
    let author = match crate::default_author(ws.as_ref().map_or(cwd, |ws| &ws.root)) {
        Ok(mut author) => {
            let id = if env("ANNOX_AUTHOR").is_some() {
                "ANNOX_AUTHOR"
            } else if config.as_ref().is_ok_and(|c| c.author.is_some()) {
                "config"
            } else {
                "git"
            };
            let name = match (env("ANNOX_AUTHOR_NAME"), author["name"].is_string()) {
                (Some(_), _) => json!("ANNOX_AUTHOR_NAME"),
                (None, true) => json!(id),
                (None, false) => Value::Null,
            };
            author["from"] = json!({ "id": id, "name": name });
            author
        }
        Err(_) => Value::Null,
    };

    let output = match env("ANNOX_OUTPUT") {
        Some(value) => json!({ "value": value, "from": "ANNOX_OUTPUT" }),
        None => match config.as_ref().ok().and_then(|c| c.output.clone()) {
            Some(value) => json!({ "value": value, "from": "config" }),
            None => json!({ "value": "text", "from": "default" }),
        },
    };

    let (config_path, credentials) = (annox_core::config::path(), annox_sync::credentials::path());
    let mut files = vec![
        file("config", config_path.clone(), config.as_ref().err().map(|e| without_path(e, config_path.as_deref()))),
        file("credentials", credentials.clone(), credentials.as_deref().and_then(|p| invalid(p, false))),
    ];
    let workspace = ws.map(|ws| {
        let marker = ws.root.join(".annox").join("annox.json");
        let text = std::fs::read_to_string(&marker).unwrap_or_default();
        let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
        files.push(file("workspace", Some(marker.clone()), invalid(&marker, true)));
        let sync = parsed["sync"]["url"].as_str().map(|url| {
            let credential = match annox_sync::credentials::lookup(url) {
                None => "missing",
                Some(c) if c.token.is_some() => "token",
                Some(_) => "none",
            };
            json!({ "url": url, "credential": credential })
        });
        json!({ "root": ws.root, "format": parsed["format"], "sync": sync })
    });

    Ok(json!({ "author": author, "output": output, "workspace": workspace, "files": files }))
}

/// A settings file: which one, where, and whether it's there and readable.
fn file(name: &str, path: Option<PathBuf>, error: Option<String>) -> Value {
    let state = match (&path, &error) {
        (_, Some(_)) => "invalid",
        (Some(p), None) if p.is_file() => "ok",
        _ => "missing",
    };
    json!({ "name": name, "path": path, "state": state, "error": error })
}

/// `error` from `config::load`, without the path it starts with.
fn without_path(error: &str, path: Option<&Path>) -> String {
    let prefix = path.map(|p| format!("{}: ", p.display())).unwrap_or_default();
    error.strip_prefix(&prefix).unwrap_or(error).to_owned()
}

/// What's wrong with the JSON file at `path`, if it exists and is broken. A
/// missing file isn't wrong, unless it's `required`, which `find` already
/// checked.
fn invalid(path: &Path, required: bool) -> Option<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => return None,
        Err(e) => return Some(e.to_string()),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(v) if v.is_object() => None,
        Ok(_) => Some("expected a JSON object".into()),
        Err(e) => Some(e.to_string()),
    }
}
