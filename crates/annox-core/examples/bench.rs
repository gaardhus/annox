//! Rough performance measurements for the spec's "to validate" questions
//! (§3 performance, §5 scale). Run with:
//!
//!     cargo run --release -p annox-core --example bench

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use annox_core::anchor::{self, Anchor};
use annox_core::event::Event;
use annox_core::ops::{self, NewAnnotation};
use annox_core::storage::{Index, Workspace};
use annox_core::text::Text;
use serde_json::{json, Map};

/// A document of roughly `chars` code points: numbered, hard-wrapped
/// sentences, so most quotes are unique.
fn document(chars: usize) -> String {
    let mut s = String::with_capacity(chars + 100);
    let mut i = 0;
    while s.len() < chars {
        s.push_str(&format!("Sentence {i} states that the bound holds for every graph.\n"));
        i += 1;
    }
    s
}

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let out = f();
    (out, start.elapsed())
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1e3)
}

fn resolution() {
    println!("## Resolution (§3.7): resolving every anchor once\n");
    println!("| doc chars | anchors | scenario | total | per anchor | steps (step:count) |");
    println!("|---:|---:|---|---:|---:|---|");
    for &chars in &[10_000usize, 100_000, 1_000_000] {
        for &n in &[100usize, 1000] {
            let raw = document(chars);
            let doc = Text::from_raw(&raw);
            let stride = raw.len() / n;
            // Anchors on "holds for every" in evenly spaced sentences (ASCII,
            // so byte and code-point offsets agree).
            let anchors: Vec<Anchor> = (0..n)
                .map(|k| {
                    let from = k * stride;
                    let at = from + raw[from..].find("holds for every").unwrap_or(0);
                    anchor::create(&doc, at, at + 15, "doc.txt")
                })
                .collect();
            let scenarios = [
                ("unchanged", raw.clone()),
                ("line inserted at top", format!("A new first line.\n{raw}")),
                ("paragraphs re-wrapped", raw.replace(" holds for", "\nholds for")),
                ("quotes deleted", raw.replace("holds for every", "applies to all")),
            ];
            for (name, text) in scenarios {
                let doc = Text::from_raw(&text);
                let (steps, elapsed) = time(|| {
                    let mut steps = BTreeMap::new();
                    for a in &anchors {
                        *steps.entry(anchor::resolve(&doc, a).step).or_insert(0) += 1;
                    }
                    steps
                });
                let steps: Vec<String> = steps.iter().map(|(s, c)| format!("{s}:{c}")).collect();
                println!(
                    "| {chars} | {n} | {name} | {} | {:.1} µs | {} |",
                    ms(elapsed),
                    elapsed.as_secs_f64() * 1e6 / n as f64,
                    steps.join(" ")
                );
            }
        }
    }
    println!();
}

fn loading() {
    println!("## Loading (§5.7.1): reading a workspace and deriving one document\n");
    println!("| annotations | events each | event files | Index::read | load + derive |");
    println!("|---:|---:|---:|---:|---:|");
    let text = Text::from_raw(&document(20_000));
    let author = json!({ "id": "mailto:bench@example.org" });
    for &(annotations, edits) in &[(100usize, 3usize), (1000, 3), (5000, 3)] {
        let dir = std::env::temp_dir().join(format!("annox-bench-{}", annox_core::new_id()));
        let ws = Workspace::init(&dir).unwrap();
        // The first annotation creates the document record and its folder.
        let first = NewAnnotation {
            kind: "comment",
            start: 0,
            end: 8,
            body: Some("hmm"),
            label: None,
            replacement: None,
            local: false,
        };
        ops::create_annotation(&ws, &Index::read(&ws), "doc.txt", &text, &first, &author).unwrap();
        let index = Index::read(&ws);
        let folder = index.events.values().next().unwrap().1.folder.clone();
        for k in 1..annotations {
            let start = (k * 7) % (text.len() - 20);
            let id = annox_core::new_id();
            let fields = Map::from_iter([
                ("kind".into(), json!("comment")),
                ("target".into(), json!(anchor::create(&text, start, start + 10, "doc.txt"))),
                ("body".into(), json!("hmm")),
            ]);
            let mut prev = id.clone();
            let create = Event {
                id: id.clone(),
                annotation: Some(id.clone()),
                document: None,
                after: vec![],
                kind: "create".into(),
                author: author.clone(),
                time: Event::now(),
                fields,
            };
            ws.write_event(&folder, &create).unwrap();
            for j in 0..edits {
                let eid = annox_core::new_id();
                let fields = Map::from_iter([("body".into(), json!(format!("edit {j}")))]);
                let edit = Event {
                    id: eid.clone(),
                    annotation: Some(id.clone()),
                    document: None,
                    after: vec![prev],
                    kind: "edit".into(),
                    author: author.clone(),
                    time: Event::now(),
                    fields,
                };
                ws.write_event(&folder, &edit).unwrap();
                prev = eid;
            }
        }
        let (index, read) = time(|| Index::read(&ws));
        let files = index.events.len();
        let (derived, derive) = time(|| index.load("doc.txt").derive().len());
        assert!(derived >= annotations - 1);
        println!("| {annotations} | {} | {files} | {} | {} |", edits + 1, ms(read), ms(derive));
        let _ = std::fs::remove_dir_all(&dir);
    }
    println!();
}

fn main() {
    resolution();
    loading();
}
