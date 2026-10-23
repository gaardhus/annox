//! Per-user configuration, shared by every annox tool on the machine and never
//! stored in `.annox/`.
//!
//! The file is `$ANNOX_CONFIG_FILE`, or `$XDG_CONFIG_HOME/annox/config.json`
//! (default `~/.config/annox/config.json`):
//!
//! ```json
//! { "author": { "id": "mailto:ada@example.org", "name": "Ada" }, "output": "json" }
//! ```
//!
//! A missing file is the same as an empty one. Unknown fields are ignored.

use std::path::PathBuf;

use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    /// The identity to write events under (§2.1, §2.7).
    pub author: Option<Value>,
    /// What the CLI prints by default: `json` or `text`.
    pub output: Option<String>,
}

/// The directory holding annox's per-user files: `$XDG_CONFIG_HOME/annox`,
/// or `~/.config/annox`.
pub fn dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("annox"))
}

pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ANNOX_CONFIG_FILE") {
        return Some(PathBuf::from(p));
    }
    Some(dir()?.join("config.json"))
}

/// Reads the config file. The error names the file and what's wrong with it.
pub fn load() -> Result<Config, String> {
    let Some(path) = path() else { return Ok(Config::default()) };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse(text: &str) -> Result<Config, String> {
    let file: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if !file.is_object() {
        return Err("expected a JSON object".into());
    }
    let author = match &file["author"] {
        Value::Null => None,
        author => {
            if !author["id"].is_string() {
                return Err("`author.id` must be a string".into());
            }
            if !matches!(author["name"], Value::Null | Value::String(_)) {
                return Err("`author.name` must be a string".into());
            }
            let mut clean = serde_json::json!({ "id": author["id"] });
            if author["name"].is_string() {
                clean["name"] = author["name"].clone();
            }
            Some(clean)
        }
    };
    let output = match &file["output"] {
        Value::Null => None,
        Value::String(o) if o == "json" || o == "text" => Some(o.clone()),
        _ => return Err("`output` must be \"json\" or \"text\"".into()),
    };
    Ok(Config { author, output })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_author() {
        let config = parse(r#"{ "author": { "id": "mailto:a@b", "name": "A", "extra": 1 }, "other": true }"#).unwrap();
        assert_eq!(config.author, Some(json!({ "id": "mailto:a@b", "name": "A" })));
        assert_eq!(parse("{}").unwrap(), Config::default());
    }

    #[test]
    fn parses_output() {
        assert_eq!(parse(r#"{ "output": "json" }"#).unwrap().output.as_deref(), Some("json"));
        assert!(parse(r#"{ "output": "yaml" }"#).is_err());
    }

    #[test]
    fn rejects_bad_author() {
        assert!(parse(r#"{ "author": { "name": "A" } }"#).is_err());
        assert!(parse(r#"{ "author": { "id": "x", "name": 3 } }"#).is_err());
        assert!(parse("[]").is_err());
        assert!(parse("not json").is_err());
    }
}
