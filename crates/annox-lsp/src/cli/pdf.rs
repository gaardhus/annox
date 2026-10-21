//! Comments in PDFs: `annox export-pdf`.

use std::path::Path;

use annox_core::anchor::{self, Anchor};
use annox_core::storage::Index;
use annox_core::text::Text;
use anyhow::Context;
use serde_json::{json, Value};

use super::{list, open_file, Filter};
use crate::pdf;

/// How much source text around a quote `export-pdf` compares, to tell
/// repeated quotes apart.
const PDF_CONTEXT: usize = 64;

pub(super) fn export_pdf(
    file: &str,
    pdf: &Path,
    output: Option<&Path>,
    closed: bool,
    filter: Filter,
    cwd: &Path,
) -> anyhow::Result<Value> {
    let (ws, path, _) = open_file(cwd, file)?;
    let pdf = cwd.join(pdf);
    let output = match output {
        Some(o) => cwd.join(o),
        None => pdf.with_extension("annotated.pdf"),
    };
    let views = list(Some(file), false, closed, None, filter, cwd)?;
    let raw = std::fs::read_to_string(ws.root.join(&path)).with_context(|| format!("can't read {file}"))?;
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
