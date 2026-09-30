//! WebAssembly bindings to `annox-core`, for the browser demo in `site/`.
//!
//! Values cross the boundary as JSON strings; `site/annox-demo/annox.js`
//! wraps them. Offsets here are UTF-16 code units, the unit of JavaScript
//! strings and DOM selections, and are converted to and from the code points
//! annox uses (§3.3). Anchors themselves keep code-point offsets, as stored.

use annox_core::anchor::{self, Anchor, Resolution, State};
use annox_core::event::Event;
use annox_core::replay;
use annox_core::suggestion;
use annox_core::text::Text;
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

/// The code-point offset of UTF-16 offset `u` in `text`, clamped to its end.
fn from_utf16(text: &Text, u: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.chars.iter().enumerate() {
        if units >= u {
            return i;
        }
        units += c.len_utf16();
    }
    text.len()
}

/// The UTF-16 offset of code-point offset `i` in `text`.
fn to_utf16(text: &Text, i: usize) -> usize {
    text.chars[..i].iter().map(|c| c.len_utf16()).sum()
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Exact => "exact",
        State::Relocated => "relocated",
        State::Orphaned => "orphaned",
    }
}

fn resolution_json(text: &Text, r: &Resolution) -> Value {
    let mut out = json!({ "state": state_name(r.state), "step": r.step });
    if let Some((start, end)) = r.range {
        out["start"] = json!(to_utf16(text, start));
        out["end"] = json!(to_utf16(text, end));
    }
    out
}

fn parse<T: serde::de::DeserializeOwned>(what: &str, s: &str) -> Result<T, String> {
    serde_json::from_str(s).map_err(|e| format!("invalid {what}: {e}"))
}

pub fn version_of(raw: &str) -> String {
    Text::from_raw(raw).version
}

pub fn create_anchor_json(raw: &str, start: usize, end: usize, path: &str) -> Result<String, String> {
    let text = Text::from_raw(raw);
    let (s, e) = (from_utf16(&text, start), from_utf16(&text, end));
    if s > e {
        return Err(format!("start {start} is after end {end}"));
    }
    Ok(json!(anchor::create(&text, s, e, path)).to_string())
}

pub fn resolve_json(raw: &str, anchor: &str) -> Result<String, String> {
    let text = Text::from_raw(raw);
    let anchor: Anchor = parse("anchor", anchor)?;
    Ok(resolution_json(&text, &anchor::resolve(&text, &anchor)).to_string())
}

pub fn apply_suggestion_json(raw: &str, anchor: &str, replacement: &str) -> Result<String, String> {
    let text = Text::from_raw(raw);
    let anchor: Anchor = parse("anchor", anchor)?;
    let out = match suggestion::apply(&text, &anchor, replacement) {
        Ok(applied) => {
            let mut out = resolution_json(&text, &applied.resolution);
            out["applied"] = json!(true);
            out["text"] = json!(applied.text.to_string());
            out["version"] = json!(applied.text.version);
            out
        }
        Err(resolution) => {
            let mut out = resolution_json(&text, &resolution);
            out["applied"] = json!(false);
            out
        }
    };
    Ok(out.to_string())
}

pub fn derive_json(events: &str) -> Result<String, String> {
    let events: Vec<Event> = parse("events", events)?;
    let mut ids: Vec<&str> = events.iter().filter_map(|e| e.annotation.as_deref()).collect();
    ids.sort_unstable();
    ids.dedup();
    let derived: Vec<Value> = ids
        .into_iter()
        .filter_map(|id| {
            let mut a = replay::derive_annotation(&events, id)?;
            a["heads"] = json!(replay::annotation_heads(&events, id));
            Some(a)
        })
        .collect();
    Ok(Value::Array(derived).to_string())
}

/// The version of `text` (§3.4).
#[wasm_bindgen]
pub fn version(text: &str) -> String {
    version_of(text)
}

/// An anchor for `[start, end)` of `text`, as JSON (§3.6).
#[wasm_bindgen(js_name = createAnchor)]
pub fn create_anchor(text: &str, start: usize, end: usize, path: &str) -> Result<String, JsError> {
    create_anchor_json(text, start, end, path).map_err(|e| JsError::new(&e))
}

/// Resolves an anchor against `text` (§3.7): `{ state, step, start?, end? }`.
#[wasm_bindgen]
pub fn resolve(text: &str, anchor: &str) -> Result<String, JsError> {
    resolve_json(text, anchor).map_err(|e| JsError::new(&e))
}

/// Applies a suggestion (§4.3): the resolution plus `applied`, and the new
/// `text` and `version` when it applied.
#[wasm_bindgen(js_name = applySuggestion)]
pub fn apply_suggestion(text: &str, anchor: &str, replacement: &str) -> Result<String, JsError> {
    apply_suggestion_json(text, anchor, replacement).map_err(|e| JsError::new(&e))
}

/// Derives every annotation in a JSON array of events (§2.5.6), each with
/// its `heads` for the `after` of the next event.
#[wasm_bindgen]
pub fn derive(events: &str) -> Result<String, JsError> {
    derive_json(events).map_err(|e| JsError::new(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_round_trip_through_utf16() {
        let text = Text::from_raw("a😀b");
        assert_eq!(from_utf16(&text, 3), 2);
        assert_eq!(to_utf16(&text, 2), 3);
    }

    #[test]
    fn anchor_follows_an_edit() {
        let anchor = create_anchor_json("say 😀 hello world", 6, 11, "a.md").unwrap();
        let r: Value = serde_json::from_str(&resolve_json("well, say 😀 hello world", &anchor).unwrap()).unwrap();
        assert_eq!(r["state"], "relocated");
        assert_eq!((r["start"].as_u64(), r["end"].as_u64()), (Some(12), Some(17)));
    }

    #[test]
    fn applies_and_derives() {
        let anchor = create_anchor_json("teh cat", 0, 3, "a.md").unwrap();
        let r: Value = serde_json::from_str(&apply_suggestion_json("teh cat", &anchor, "the").unwrap()).unwrap();
        assert_eq!((r["applied"].as_bool(), r["text"].as_str()), (Some(true), Some("the cat")));

        let events = json!([{
            "id": "01", "annotation": "01", "after": [], "type": "create",
            "author": { "id": "urn:x" }, "time": "2026-09-30T00:00:00Z",
            "kind": "suggestion", "target": serde_json::from_str::<Value>(&anchor).unwrap(),
            "edit": { "replacement": "the" }
        }]);
        let d: Value = serde_json::from_str(&derive_json(&events.to_string()).unwrap()).unwrap();
        assert_eq!(d[0]["status"], "open");
        assert_eq!(d[0]["heads"], json!(["01"]));
    }
}
