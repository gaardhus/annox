//! Runs the conformance test vectors in `spec/tests/` (§1.5).

use std::collections::BTreeMap;
use std::path::PathBuf;

use annox_core::anchor::{self, Anchor, State};
use annox_core::event::Event;
use annox_core::replay;
use annox_core::storage::Index;
use annox_core::suggestion;
use annox_core::text::Text;
use serde_json::{json, Value};

fn vectors(name: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/tests").join(format!("{name}.json"));
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    file["cases"].as_array().unwrap().clone()
}

fn check_all(name: &str, check: impl Fn(&Value) -> Result<(), String>) {
    let cases = vectors(name);
    let failures: Vec<String> =
        cases.iter().filter_map(|c| check(c).err().map(|e| format!("{}: {e}", c["name"].as_str().unwrap()))).collect();
    assert!(
        failures.is_empty(),
        "{} of {} {name} vectors failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

fn state_name(s: State) -> &'static str {
    match s {
        State::Exact => "exact",
        State::Relocated => "relocated",
        State::Orphaned => "orphaned",
    }
}

fn expect_eq(what: &str, got: Value, want: &Value) -> Result<(), String> {
    if &got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got}, want {want}"))
    }
}

#[test]
fn anchoring() {
    check_all("anchoring", |c| {
        let doc = Text::from_raw(c["document"].as_str().unwrap());
        let anchor: Anchor = serde_json::from_value(c["anchor"].clone()).unwrap();
        let r = anchor::resolve(&doc, &anchor);
        let got = json!({
            "state": state_name(r.state),
            "start": r.range.map(|(s, _)| s),
            "end": r.range.map(|(_, e)| e),
        });
        expect_eq("resolution", got, &c["expected"])
    });
}

#[test]
fn suggestions() {
    check_all("suggestions", |c| {
        let doc = Text::from_raw(c["document"].as_str().unwrap());
        let s = &c["suggestion"];
        let target: Anchor = serde_json::from_value(s["target"].clone()).unwrap();
        let replacement = s["edit"]["replacement"].as_str().unwrap();
        let got = match suggestion::apply(&doc, &target, replacement) {
            Ok(applied) => json!({
                "applicability": "applicable",
                "resolutionStep": applied.resolution.step,
                "result": applied.text.to_string(),
            }),
            Err(r) => json!({ "applicability": "stale", "resolutionStep": r.step, "result": null }),
        };
        expect_eq("application", got, &c["expected"])
    });
}

#[test]
fn replay() {
    check_all("replay", |c| {
        let events: Vec<Event> = serde_json::from_value(c["events"].clone()).unwrap();
        let id = c["annotation"].as_str().unwrap();
        let got = replay::derive_annotation(&events, id).ok_or("no annotation derived")?;
        expect_eq("state", got.clone(), &c["expected"])?;
        let mut reversed = events.clone();
        reversed.reverse();
        expect_eq("state with reversed events", replay::derive_annotation(&reversed, id).unwrap(), &got)
    });
}

#[test]
fn storage() {
    check_all("storage", |c| {
        let files: BTreeMap<String, String> = serde_json::from_value(c["files"].clone()).unwrap();
        let index = Index::read(&files);
        let loaded = index.load(c["path"].as_str().unwrap());
        let documents: serde_json::Map<String, Value> = index
            .documents
            .iter()
            .map(|(id, d)| {
                (id.clone(), json!({ "path": d.path, "mergedInto": d.merged_into, "conflicts": d.conflicts }))
            })
            .collect();
        let mut events: Vec<&str> = loaded.events.iter().map(|e| e.id.as_str()).collect();
        events.sort_unstable();
        let got = json!({
            "documents": documents,
            "at": loaded.at,
            "loaded": loaded.loaded,
            "folders": loaded.folders,
            "events": events,
            "annotations": loaded.annotations,
        });
        expect_eq("load", got, &c["expected"])
    });
}
