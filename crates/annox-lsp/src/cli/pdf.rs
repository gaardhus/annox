//! Comments in PDFs: `annox export-pdf`, `annox import-pdf` and `annox sync-pdf`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use annox_core::anchor::{self, Anchor};
use annox_core::ops::{self, NewAnnotation};
use annox_core::storage::Index;
use annox_core::text::Text;
use anyhow::{bail, Context};
use serde_json::{json, Value};

use super::watch::WATCH_INTERVAL;
use super::{author, line_of, list, open_file, Author, Filter, SyncPdf};
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
    let index = Index::read(&ws);
    let exported = pdf::export(&threads, &path, &pdf, &output, &owned_by_annox(&index))?;
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
    // PDF comments imported before, by their identity (`pdf::Found::key`):
    // what each became in this document, as the same PDF may be imported
    // into another too, such as its source; and for replies, the threads
    // they're in, which may be another document's.
    let mut imported: BTreeMap<String, String> = BTreeMap::new();
    let mut replied: BTreeSet<(String, String)> = BTreeSet::new();
    let here = index.documents_at(&path);
    // Authors by name and by id, the latest of each.
    let mut authors: BTreeMap<String, Value> = BTreeMap::new();
    for (id, (event, loc)) in &index.events {
        let pdf = event.field("ext").map(|ext| &ext[EXT]["pdf"]);
        if let Some(k) = pdf.and_then(|pdf| pdf["key"].as_str()).map(str::to_owned) {
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
    let exported = exported_id;

    let (mut added, mut skipped, mut unchanged) = (Vec::new(), Vec::new(), 0);
    let (roots, replies): (Vec<_>, Vec<_>) = found.iter().partition(|f| matches!(f.what, pdf::What::Root { .. }));
    for f in roots.into_iter().chain(replies) {
        let k = f.key.clone();
        // A thread's root that annox wrote: what a reviewer added to its
        // note, in a viewer that can't reply, such as Okular, is a reply.
        if let (Some(id), pdf::What::Root { .. }) = (exported(&index, &f.name).filter(|id| *id == f.name), &f.what) {
            match added_to_note(f) {
                Added::Nothing => unchanged += 1,
                Added::Changed => skipped.push(json!({
                    "page": f.page,
                    "body": f.body,
                    "reason": format!("the note of {id} was changed, not added to; only what's added becomes a reply"),
                })),
                Added::Reply(body) => {
                    let key = format!("{k}+{body}");
                    if !replied.insert((id.clone(), key.clone())) {
                        unchanged += 1;
                        continue;
                    }
                    let source = json!({ "file": pdf_name, "name": f.name, "key": key });
                    let origin = ops::Origin { time: None, ext: Some(json!({ EXT: { "pdf": source } })) };
                    let reply = if dry_run {
                        key
                    } else {
                        let event = ops::create_reply_from(&ws, &index, &id, &body, local, &fallback, &origin)?;
                        index = Index::read(&ws);
                        event.id
                    };
                    added.push(json!({ "id": reply, "kind": "reply", "parent": id, "author": display(&fallback) }));
                }
            }
            continue;
        }
        if exported(&index, &f.name).is_some() || imported.contains_key(&k) {
            unchanged += 1;
            continue;
        }
        let who = author_of(f.author.as_deref());
        let source = json!({ "file": pdf_name, "name": f.name, "key": f.key });
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
                let Some(parent) = exported(&index, &parent.name).or_else(|| imported.get(&parent.key).cloned()) else {
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

/// Imports the comments made on the PDF of `options`, then writes the
/// document's open threads into it, replacing what an earlier sync wrote.
pub(super) fn sync_pdf(options: &SyncPdf, cwd: &Path) -> anyhow::Result<Value> {
    let SyncPdf { file, pdf, local, author: who, .. } = options;
    let (_, _, abs) = open_file(cwd, file)?;
    let pdf = pdf_of(file, &abs, pdf.as_deref(), cwd)?;
    let imported = import_pdf(file, Some(&pdf), false, *local, who, cwd)?;
    let exported = export_pdf(file, Some(&pdf), Some(&pdf), false, Filter::default(), cwd)?;
    Ok(json!({
        "path": imported["path"],
        "pdf": imported["pdf"],
        "added": imported["added"],
        "skipped": imported["skipped"],
        "placed": exported["placed"],
        "unplaced": exported["unplaced"],
    }))
}

/// Runs `sync-pdf` with `options` until `stop` is set: once at the start,
/// and again once the document's threads or the PDF have changed and then
/// stayed the same for `settle`. Passes what each sync did to `emit`. With a
/// hub in the workspace's `annox.json`, it syncs with it too, so others'
/// threads reach the PDF. A sync that fails, such as when a viewer holds the
/// PDF locked, is printed and tried again; an error from `emit` ends it.
pub fn watch_pdf(
    options: SyncPdf,
    cwd: &Path,
    stop: &AtomicBool,
    mut emit: impl FnMut(Value) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let cwd = cwd.canonicalize()?;
    let (ws, _, abs) = open_file(&cwd, &options.file)?;
    let pdf = pdf_of(&options.file, &abs, options.pdf.as_deref(), &cwd)?;
    let settle = Duration::from_millis(options.settle);
    let (hub, from_hub) = crossbeam_channel::unbounded();
    let replica = match crate::sync::replica_config(&ws, || author(&options.author, &ws).unwrap_or_default()) {
        Some(Ok(config)) => {
            eprintln!("annox: syncing with {}", config.url);
            Some(annox_sync::replica::spawn(config, hub))
        }
        Some(Err(message)) => {
            eprintln!("{message}");
            None
        }
        None => None,
    };
    let scan = || {
        if let Some(tx) = &replica {
            let _ = tx.send(annox_sync::replica::ToReplica::Scan);
        }
    };
    // The document's threads as they'd be written into the PDF, without
    // what changes when someone reads them.
    let threads = || -> anyhow::Result<String> {
        let mut views = list(Some(&options.file), false, false, None, Filter::default(), &cwd)?;
        for view in views.as_array_mut().into_iter().flatten() {
            view.as_object_mut().map(|v| v.remove("unread"));
            for reply in view["replies"].as_array_mut().into_iter().flatten() {
                reply.as_object_mut().map(|r| r.remove("unread"));
            }
        }
        Ok(views.to_string())
    };
    let stamp = || std::fs::metadata(&pdf).ok().map(|m| (m.len(), m.modified().ok()));

    let mut fingerprint = None;
    let mut shape = String::new();
    let mut synced = None;
    let mut pending: Option<(_, Instant)> = None;
    while !stop.load(Ordering::Relaxed) {
        for message in from_hub.try_iter() {
            if let annox_sync::replica::FromReplica::Status { message, .. } = message {
                eprintln!("annox sync: {message}");
            }
        }
        let print = crate::fingerprint(&ws.root);
        if fingerprint.as_ref() != Some(&print) {
            fingerprint = Some(print);
            scan();
            match threads() {
                Ok(s) => shape = s,
                Err(e) => eprintln!("annox sync-pdf: {e:#}"),
            }
        }
        let state = (shape.clone(), stamp());
        if synced.as_ref() != Some(&state) {
            let now = Instant::now();
            let since = match &pending {
                Some((s, since)) if *s == state => *since,
                _ => {
                    pending = Some((state.clone(), now));
                    now
                }
            };
            if synced.is_none() || now.duration_since(since) >= settle {
                match sync_pdf(&options, &cwd) {
                    Ok(out) => {
                        emit(out)?;
                        // What the sync itself changed isn't a reason to sync again.
                        fingerprint = Some(crate::fingerprint(&ws.root));
                        scan();
                        shape = threads().unwrap_or_default();
                        synced = Some((shape.clone(), stamp()));
                        pending = None;
                    }
                    Err(e) => {
                        eprintln!("annox sync-pdf: {e:#}");
                        pending = Some((state, now));
                    }
                }
            }
        }
        std::thread::sleep(WATCH_INTERVAL);
    }
    if let Some(tx) = replica {
        let _ = tx.send(annox_sync::replica::ToReplica::Stop);
    }
    Ok(())
}

/// The annox id of a PDF comment `export-pdf` wrote: it's named after it,
/// maybe with a suffix, as in `<id>-caret`.
fn exported_id(index: &Index, name: &str) -> Option<String> {
    let id = name.get(..36).filter(|_| name.len() == 36 || name[36..].starts_with('-'))?;
    index.events.contains_key(id).then(|| id.to_owned())
}

/// Which PDF comments are annox's, for `export-pdf` to replace: the ones it
/// wrote, and the ones imported into any document, which it writes itself.
fn owned_by_annox(index: &Index) -> impl Fn(&str, &str) -> bool + '_ {
    let imported: BTreeSet<String> = index
        .events
        .values()
        .filter_map(|(event, _)| {
            let pdf = &event.field("ext")?[EXT]["pdf"];
            Some(pdf["key"].as_str()?.to_owned())
        })
        .collect();
    move |name, key| exported_id(index, name).is_some() || imported.contains(key)
}

/// What a reviewer did to the note of a thread's root that annox wrote.
enum Added {
    Nothing,
    /// Wrote this at the end of it.
    Reply(String),
    /// Changed what annox wrote.
    Changed,
}

fn added_to_note(f: &pdf::Found) -> Added {
    // Without annox's copy of the note, as when a viewer dropped it, there's
    // no telling what was added.
    let Some(written) = &f.written else { return Added::Nothing };
    let now = f.body.as_deref().unwrap_or_default().trim_end();
    match now.strip_prefix(written.trim_end()) {
        Some(rest) if rest.trim().is_empty() => Added::Nothing,
        Some(rest) => Added::Reply(rest.trim().to_owned()),
        None => Added::Changed,
    }
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
