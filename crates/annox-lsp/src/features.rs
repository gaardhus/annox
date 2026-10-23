//! The standard LSP features, for plain clients (§6.5): diagnostics, hover,
//! and code actions and commands.

use annox_core::anchor;
use annox_core::suggestion;
use lsp_server::{Notification, RequestId};
use lsp_types::notification::{Notification as _, PublishDiagnostics};
use lsp_types::{
    CodeAction, CodeActionOrCommand, CodeActionParams, Command, Diagnostic, DiagnosticSeverity, ExecuteCommandParams,
    Hover, HoverContents, HoverParams, MarkupContent, MarkupKind, NumberOrString, PublishDiagnosticsParams, Url,
};
use serde_json::{json, Map, Value};

use crate::markdown::thread_markdown;
use crate::position::LineIndex;
use crate::{
    fail, Analysis, Item, Server, Work, INVALID_OPERATION, INVALID_PARAMS, STALE_SUGGESTION, UNKNOWN_ANNOTATION,
};

impl Server<'_> {
    pub(crate) fn send_diagnostics(&self, uri: Url, diagnostics: Vec<Diagnostic>) {
        let params = PublishDiagnosticsParams { uri, diagnostics, version: None };
        self.send(Notification::new(PublishDiagnostics::METHOD.into(), params));
    }

    pub(crate) fn diagnostics_for(&self, a: &Analysis) -> Vec<Diagnostic> {
        let lines = LineIndex::new(&a.text, self.encoding);
        let mut out = Vec::new();
        let mut orphaned = 0;
        for item in a.items.iter().filter(|i| i.is_open()) {
            let Some((start, end)) = item.resolution.range else {
                orphaned += 1;
                if let Some(s) = item.suggested {
                    out.push(Diagnostic {
                        range: lines.range(s.range.0, s.range.1),
                        severity: Some(DiagnosticSeverity::HINT),
                        code: Some(NumberOrString::String(item.kind().to_owned())),
                        source: Some("annox".into()),
                        message: format!("Orphaned {} may belong here: {}", item.kind(), summary(item)),
                        data: Some(json!({ "annotation": item.id, "suggested": true })),
                        ..Default::default()
                    });
                }
                continue;
            };
            let (mut severity, mut message) = if item.kind() == "suggestion" {
                let q = &item.target.selectors.quote.exact;
                let r = item.state["edit"]["replacement"].as_str().unwrap_or_default();
                let text = format!("Suggestion: \"{q}\" → \"{r}\"");
                let text = if item.applicable() { text } else { format!("Stale {text}") };
                (DiagnosticSeverity::INFORMATION, text)
            } else {
                (DiagnosticSeverity::HINT, summary(item))
            };
            if item.conflicted() {
                severity = DiagnosticSeverity::WARNING;
                message = format!("Conflict: {message}");
            }
            out.push(Diagnostic {
                range: lines.range(start, end),
                severity: Some(severity),
                code: Some(NumberOrString::String(item.kind().to_owned())),
                source: Some("annox".into()),
                message,
                data: Some(json!({ "annotation": item.id })),
                ..Default::default()
            });
        }
        if orphaned > 0 {
            out.push(Diagnostic {
                range: lines.range(0, 0),
                severity: Some(DiagnosticSeverity::WARNING),
                source: Some("annox".into()),
                message: format!("{orphaned} annotation{} could not be located", if orphaned == 1 { "" } else { "s" }),
                ..Default::default()
            });
        }
        out
    }

    // Hover (§6.5.2).

    pub(crate) fn hover(&self, p: HoverParams) -> Option<Hover> {
        let doc = &p.text_document_position_params;
        let a = self.analyze(&doc.text_document.uri)?;
        let lines = LineIndex::new(&a.text, self.encoding);
        let offset = lines.offset(doc.position);
        // Orphans are shown at their suggested location (§3.7.4).
        let at = |i: &Item| i.resolution.range.or(i.suggested.map(|s| s.range));
        let hits: Vec<&Item> = a
            .items
            .iter()
            .filter(|i| i.is_open())
            .filter(|i| at(i).is_some_and(|(s, e)| s <= offset && offset <= e))
            .collect();
        let first = at(hits.first()?)?;
        let sections: Vec<String> = hits.iter().map(|i| thread_markdown(i, a.replies.get(&i.id))).collect();
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: sections.join("\n\n---\n\n"),
            }),
            range: Some(lines.range(first.0, first.1)),
        })
    }

    // Code actions and commands (§6.5.3).

    pub(crate) fn code_actions(&self, p: CodeActionParams) -> Vec<CodeActionOrCommand> {
        let Some(a) = self.analyze(&p.text_document.uri) else { return vec![] };
        let lines = LineIndex::new(&a.text, self.encoding);
        let (from, to) = (lines.offset(p.range.start), lines.offset(p.range.end));
        let mut actions = Vec::new();
        for item in &a.items {
            let Some((s, e)) = item.resolution.range.or(item.suggested.map(|s| s.range)) else { continue };
            if s > to || e < from || item.deleted() {
                continue;
            }
            for (command, title) in offered_commands(item) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: title.into(),
                    command: Some(Command {
                        title: title.into(),
                        command: command.into(),
                        arguments: Some(vec![json!({ "annotation": item.id })]),
                    }),
                    ..Default::default()
                }));
            }
        }
        actions
    }

    pub(crate) fn execute(&mut self, id: RequestId, p: ExecuteCommandParams) {
        let annotation = p.arguments.first().and_then(|a| a.get("annotation")).and_then(Value::as_str);
        let Some(annotation) = annotation.map(str::to_owned) else {
            return self.reply(id, fail(INVALID_PARAMS, "missing annotation argument"));
        };
        let Some((uri, a)) = self.find(&annotation) else {
            return self.reply(id, fail(UNKNOWN_ANNOTATION, format!("unknown annotation {annotation}")));
        };
        let Some(item) = a.item(&annotation) else {
            return self.reply(id, fail(INVALID_OPERATION, "commands apply to comments and suggestions"));
        };
        if !offered_commands(item).iter().any(|(c, _)| *c == p.command) {
            return self.reply(id, fail(INVALID_OPERATION, format!("{} does not apply to this annotation", p.command)));
        }
        let status = match p.command.as_str() {
            "annox.accept" => return self.accept(id, uri, &a, &annotation, false),
            "annox.reattachSuggested" => {
                let (start, end) = item.suggested.map(|s| s.range).unwrap_or_default();
                let target = anchor::create(&a.text, start, end, &a.rel);
                let fields = Map::from_iter([("target".into(), json!(target))]);
                let result = self.write_event(&a, &annotation, "reanchor", fields);
                self.reply(id, result.map(|_| Value::Null));
                return self.refresh_all();
            }
            "annox.reject" => "rejected",
            "annox.resolve" => "resolved",
            _ => "open",
        };
        let result = self.write_event(&a, &annotation, "status", Map::from_iter([("status".into(), json!(status))]));
        self.reply(id, result.map(|_| Value::Null));
        self.refresh_all();
    }

    /// Applies a suggestion through `workspace/applyEdit`; the status event
    /// is written when the client confirms (§6.6.2).
    pub(crate) fn accept(&mut self, command: RequestId, uri: Url, a: &Analysis, annotation: &str, respond_view: bool) {
        let Some(item) = a.item(annotation).filter(|i| i.kind() == "suggestion" && i.is_open()) else {
            return self.reply(command, fail(INVALID_OPERATION, "only open suggestions can be accepted"));
        };
        let Some(replacement) = item.state["edit"]["replacement"].as_str().map(str::to_owned) else {
            return self.reply(command, fail(INVALID_OPERATION, "the suggestion has no replacement"));
        };
        match suggestion::apply(&a.text, &item.target, &replacement) {
            Ok(applied) => {
                let work = Work::Accept { annotation: annotation.to_owned() };
                self.apply_edit(command, uri, vec![(applied.start, applied.end, replacement)], work, respond_view);
            }
            Err(_) => self.reply(command, fail(STALE_SUGGESTION, "the suggestion is stale")),
        }
    }
}

/// The commands offered for an annotation (§6.5.3).
fn offered_commands(item: &Item) -> Vec<(&'static str, &'static str)> {
    match (item.kind(), item.status()) {
        ("suggestion", "open") if item.applicable() => {
            vec![("annox.accept", "Accept suggestion"), ("annox.reject", "Reject suggestion")]
        }
        ("suggestion", "open") => vec![("annox.reject", "Reject suggestion")],
        ("suggestion", "rejected" | "withdrawn") => vec![("annox.reopen", "Reopen suggestion")],
        ("comment", "open") if item.suggested.is_some() => {
            vec![("annox.reattachSuggested", "Re-attach orphaned comment here"), ("annox.resolve", "Resolve thread")]
        }
        ("comment", "open") => vec![("annox.resolve", "Resolve thread")],
        ("comment", "resolved") => vec![("annox.reopen", "Reopen thread")],
        _ => vec![],
    }
}

/// A comment's first line of body, or its label, for diagnostics.
fn summary(item: &Item) -> String {
    if item.kind() == "suggestion" {
        let q = &item.target.selectors.quote.exact;
        let r = item.state["edit"]["replacement"].as_str().unwrap_or_default();
        return format!("\"{q}\" → \"{r}\"");
    }
    let body = item.state["body"].as_str().and_then(|b| b.lines().next()).filter(|l| !l.is_empty());
    body.or(item.state["label"].as_str()).unwrap_or("Highlight").to_owned()
}
