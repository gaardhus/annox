//! Comments in PDFs: `annox export-pdf` and `annox import-pdf`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use annox_core::anchor::{self, Anchor};
use annox_core::ops::{self, NewAnnotation};
use annox_core::storage::Index;
use annox_core::text::Text;
use anyhow::{bail, Context};
use serde_json::{json, Value};

use super::{author, line_of, list, open_file, Author, Filter};
use crate::pdf;

/// How much source text around a quote `export-pdf` compares, to tell
/// repeated quotes apart.
const PDF_CONTEXT: usize = 64;

/// The PDF argument of `export-pdf` and `import-pdf`: `pdf`, or the document
/// at `abs` if it's a PDF.
fn pdf_of(file: &str, abs: &Path, pdf: Option<&Path>, cwd: &Path) -> anyhow::Result<PathBuf> {
    match pdf {
        Some(pdf) => Ok(cwd.join(pdf)),
        None if pdf::is_pdf(file) => Ok(abs.to_owned()),
        None => bail!("{file} isn't a PDF; pass the PDF compiled from it too"),
    }
}

pub(super) fn export_pdf(
    file: &str,
    pdf: Option<&Path>,
    output: Option<&Path>,
    closed: bool,
    filter: Filter,
    cwd: &Path,
) -> anyhow::Result<Value> {
    let (ws, path, abs) = open_file(cwd, file)?;
    let pdf = pdf_of(file, &abs, pdf, cwd)?;
    let output = match output {
        Some(o) => cwd.join(o),
        None => pdf.with_extension("annotated.pdf"),
    };
    let views = list(Some(file), false, closed, None, filter, cwd)?;
    let raw = pdf::read_document(&ws.root.join(&path)).with_context(|| format!("can't read {file}"))?;
    let text = Text::from_raw(&raw);
    let states = Index::read(&ws).load(&path).derive();
    let name = |author: &Value| author["name"].as_str().or(author["id"].as_str()).unwrap_or_default().to_owned();
    let mut threads = Vec::new();
    for view in views.as_array().into_iter().flatten() {
        let id = view["id"].as_str().unwrap_or_default();
        let Some(target) = states.get(id).and_then(|s| serde_json::from_value::<Anchor>(s["target"].clone()).ok())
        else {
            continue;
        };
        let len = text.len().max(1) as f64;
        let (quote, prefix, suffix, at) = match anchor::resolve(&text, &target).range {
            Some((start, end)) => (
                text.slice(start, end),
                text.slice(start.saturating_sub(PDF_CONTEXT), start),
                text.slice(end, (end + PDF_CONTEXT).min(text.len())),
                start as f64 / len,
            ),
            // Not in the source any more, but the PDF may be older than the source.
            None => {
                let q = target.selectors.quote;
                (q.exact, q.prefix, q.suffix, target.selectors.position.start as f64 / len)
            }
        };
        let replies = view["replies"].as_array().into_iter().flatten();
        threads.push(pdf::Thread {
            id: id.to_owned(),
            author: name(&view["author"]),
            created: view["created"].as_str().unwrap_or_default().to_owned(),
            body: view["body"].as_str().map(str::to_owned),
            replacement: view["replacement"].as_str().map(str::to_owned),
            quote,
            prefix,
            suffix,
            at: at.min(1.0),
            line: view["line"].as_u64().map(|l| l as usize),
            replies: replies
                .map(|r| pdf::Reply {
                    id: r["id"].as_str().unwrap_or_default().to_owned(),
                    author: name(&r["author"]),
                    created: r["created"].as_str().unwrap_or_default().to_owned(),
                    body: r["body"].as_str().unwrap_or_default().to_owned(),
                })
                .collect(),
        });
    }
    let exported = pdf::export(&threads, &path, &pdf, &output)?;
    Ok(json!({ "output": output, "placed": exported.placed, "unplaced": exported.unplaced }))
}

/// Where `import-pdf` records which PDF comment an annotation or reply came
/// from (§2.9).
const EXT: &str = "io.github.gaardhus.annox";

pub(super) fn import_pdf(
    file: &str,
    pdf: Option<&Path>,
    dry_run: bool,
    local: bool,
    who: &Author,
    cwd: &Path,
) -> anyhow::Result<Value> {
    let (ws, path, abs) = open_file(cwd, file)?;
    let pdf_path = pdf_of(file, &abs, pdf, cwd)?;
    let raw = pdf::read_document(&abs).with_context(|| format!("can't read {file}"))?;
    let text = Text::from_raw(&raw);
    let found = pdf::import(&pdf_path, &text.to_string(), pdf::Syntax::of(&path))?;
    let pdf_name = pdf_path
        .canonicalize()
        .ok()
        .and_then(|p| ws.relative(&p))
        .unwrap_or_else(|| pdf_path.file_name().unwrap_or_default().to_string_lossy().into_owned());

    let mut index = Index::read(&ws);
    // PDF comments imported before, by their name and when they were made:
    // viewers such as PyMuPDF number their comments, so names alone repeat
    // from one PDF to the next. What each became in this document, as the
    // same PDF may be imported into another too, such as its source; and
    // for replies, the threads they're in, which may be another document's.
    let key = |name: &str, created: Option<&str>| format!("{name}@{}", created.unwrap_or_default());
    let mut imported: BTreeMap<String, String> = BTreeMap::new();
    let mut replied: BTreeSet<(String, String)> = BTreeSet::new();
    let here = index.documents_at(&path);
    // Authors by name and by id, the latest of each.
    let mut authors: BTreeMap<String, Value> = BTreeMap::new();
    for (id, (event, loc)) in &index.events {
        let pdf = event.field("ext").map(|ext| &ext[EXT]["pdf"]);
        if let Some(name) = pdf.and_then(|pdf| pdf["name"].as_str()) {
            let k = key(name, pdf.and_then(|pdf| pdf["created"].as_str()));
            match event.field("parent").and_then(Value::as_str) {
                Some(parent) => {
                    replied.insert((parent.to_owned(), k));
                }
                None if here.iter().any(|d| d == index.canonical(&loc.document)) => {
                    imported.insert(k, event.annotation.clone().unwrap_or_else(|| id.clone()));
                }
                None => {}
            }
        }
        for key in ["name", "id"] {
            if let Some(k) = event.author[key].as_str() {
                authors.insert(k.to_owned(), event.author.clone());
            }
        }
    }
    let fallback = author(who, &ws)?;
    let author_of = |name: Option<&str>| match name {
        Some(name) => authors.get(name).cloned().unwrap_or_else(|| json!({ "id": pdf_author_id(name), "name": name })),
        None => fallback.clone(),
    };
    let display = |author: &Value| author["name"].as_str().or(author["id"].as_str()).unwrap_or_default().to_owned();
    // The annox id of a comment `export-pdf` wrote: it's named after it,
    // maybe with a suffix, as in `<id>-caret`.
    let exported = |index: &Index, name: &str| -> Option<String> {
        let id = name.get(..36).filter(|_| name.len() == 36 || name[36..].starts_with('-'))?;
        index.events.contains_key(id).then(|| id.to_owned())
    };

    let (mut added, mut skipped, mut unchanged) = (Vec::new(), Vec::new(), 0);
    let (roots, replies): (Vec<_>, Vec<_>) = found.iter().partition(|f| matches!(f.what, pdf::What::Root { .. }));
    for f in roots.into_iter().chain(replies) {
        let k = key(&f.name, f.time.as_deref());
        if exported(&index, &f.name).is_some() || imported.contains_key(&k) {
            unchanged += 1;
            continue;
        }
        let who = author_of(f.author.as_deref());
        let source = json!({ "file": pdf_name, "name": f.name, "created": f.time });
        let origin = ops::Origin { time: f.time.clone(), ext: Some(json!({ EXT: { "pdf": source } })) };
        let mut skip = |reason: String, quote: Option<&str>| {
            let mut s = json!({ "page": f.page, "author": f.author, "reason": reason });
            if let Some(quote) = quote {
                s["quote"] = json!(quote);
            }
            if let Some(body) = &f.body {
                s["body"] = json!(body);
            }
            skipped.push(s);
        };
        match &f.what {
            pdf::What::Root { range, quote, replacement } => {
                let Some((start, end)) = *range else {
                    skip(format!("its text isn't in {path}"), Some(quote));
                    continue;
                };
                if replacement.as_deref().is_some_and(|r| start == end && r.is_empty()) {
                    skip("it inserts nothing".into(), None);
                    continue;
                }
                let replacement = replacement.as_deref().map(annox_core::text::normalize);
                let kind = if replacement.is_some() { "suggestion" } else { "comment" };
                let new = NewAnnotation {
                    kind,
                    start,
                    end,
                    body: f.body.as_deref(),
                    label: None,
                    replacement: replacement.as_deref(),
                    local,
                    reverts: None,
                };
                let id = if dry_run {
                    f.name.clone()
                } else {
                    let event = ops::create_annotation_from(&ws, &index, &path, &text, &new, &who, &origin)?;
                    index = Index::read(&ws);
                    event.id
                };
                imported.insert(k, id.clone());
                let mut view = json!({
                    "id": id, "kind": kind, "line": line_of(&text, start), "author": display(&who),
                    "quote": text.slice(start, end),
                });
                if let Some(r) = replacement {
                    view["replacement"] = json!(r);
                }
                added.push(view);
            }
            pdf::What::Reply { parent } => {
                let Some(body) = f.body.as_deref() else {
                    skip("it's empty".into(), None);
                    continue;
                };
                let Some(parent) = exported(&index, &parent.name)
                    .or_else(|| imported.get(&key(&parent.name, parent.time.as_deref())).cloned())
                else {
                    skip("what it replies to wasn't imported".into(), None);
                    continue;
                };
                if !replied.insert((parent.clone(), k)) {
                    unchanged += 1;
                    continue;
                }
                let id = if dry_run {
                    f.name.clone()
                } else {
                    let event = ops::create_reply_from(&ws, &index, &parent, body, local, &who, &origin)?;
                    index = Index::read(&ws);
                    event.id
                };
                added.push(json!({ "id": id, "kind": "reply", "parent": parent, "author": display(&who) }));
            }
        }
    }
    let mut out = json!({ "path": path, "pdf": pdf_name, "added": added, "skipped": skipped, "unchanged": unchanged });
    if dry_run {
        out["dryRun"] = json!(true);
    }
    Ok(out)
}

/// The author id of someone known only by the name a PDF gives them.
fn pdf_author_id(name: &str) -> String {
    let mut id = String::from("urn:annox:pdf:");
    for b in name.trim().bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' => id.push(b as char),
            _ => id.push_str(&format!("%{b:02X}")),
        }
    }
    id
}
