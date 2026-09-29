//! The `annox/*` extension methods (§6.6.2).

use annox_core::anchor::{self, Anchor};
use annox_core::ops::{self, NewAnnotation};
use annox_core::replay;
use annox_core::storage::{Index, Workspace};
use annox_core::suggestion;
use lsp_server::RequestId;
use lsp_types::{Range, Url};
use serde_json::{json, Map, Value};

use crate::position::LineIndex;
use crate::{
    fail, views, Analysis, Failure, PendingInit, Server, Work, CREATE_WORKSPACE, INTERNAL_ERROR, INVALID_OPERATION,
    INVALID_PARAMS, NEEDS_REVIEW, NOT_CONFLICTED, NO_WORKSPACE, OVERLAP, STALE_SUGGESTION, UNKNOWN_ANNOTATION,
};

fn str_param<'p>(params: &'p Value, name: &str) -> Result<&'p str, Failure> {
    params[name].as_str().ok_or((INVALID_PARAMS, format!("missing {name}")))
}

fn uri_param(params: &Value, name: &str) -> Result<Url, Failure> {
    Url::parse(str_param(params, name)?).map_err(|e| (INVALID_PARAMS, e.to_string()))
}

fn range_param(params: &Value) -> Result<Range, Failure> {
    serde_json::from_value(params["range"].clone()).map_err(|e| (INVALID_PARAMS, format!("range: {e}")))
}

fn io_failure(e: std::io::Error) -> Failure {
    (INTERNAL_ERROR, e.to_string())
}

/// Whether two ranges share text, or a point range touches the other range.
fn overlaps((s1, e1): (usize, usize), (s2, e2): (usize, usize)) -> bool {
    (s1 < e2 && s2 < e1) || (s1 == e1 && s2 <= s1 && s1 <= e2) || (s2 == e2 && s1 <= s2 && s2 <= e1)
}

impl Server<'_> {
    /// Dispatches an extension request. Requests that apply edits reply later
    /// (§6.6.2 "Applying edits"); all others reply here.
    pub(crate) fn extension(&mut self, id: RequestId, method: String, params: Value) {
        let result = match method.as_str() {
            "annox/accept" => return self.m_accept(id, &params),
            "annox/acceptAll" => return self.m_accept_all(id, &params),
            "annox/resolveConflict" => return self.m_resolve_conflict(id, &params),
            "annox/create" => return self.m_create_or_init(id, params),
            "annox/annotations" => self.m_annotations(&params),
            "annox/reply" => self.m_reply(&params),
            "annox/publish" => self.m_publish(&params),
            "annox/edit" => self.m_edit(&params),
            "annox/setStatus" => self.m_set_status(&params),
            "annox/retarget" => self.m_retarget(&params),
            "annox/reattach" => self.m_reattach(&params),
            "annox/delete" => self.m_simple(&params, "delete"),
            "annox/restore" => self.m_simple(&params, "restore"),
            "annox/moveDocument" => self.m_move_document(&params),
            "annox/history" => self.m_history(&params),
            _ => fail(-32601, format!("unknown method {method}")),
        };
        let changed = !matches!(method.as_str(), "annox/annotations" | "annox/history");
        self.reply(id, result);
        if changed {
            self.refresh_all();
        }
    }

    /// The open document holding `id`, analyzed.
    fn locate(&self, id: &str) -> Result<(Url, std::rc::Rc<Analysis>), Failure> {
        self.find(id).ok_or((UNKNOWN_ANNOTATION, format!("unknown annotation {id}")))
    }

    fn analyze_uri(&self, uri: &Url) -> Result<std::rc::Rc<Analysis>, Failure> {
        self.analyze(uri).ok_or((NO_WORKSPACE, "the document is not in an annox workspace".to_owned()))
    }

    fn offsets(&self, a: &Analysis, range: Range) -> (usize, usize) {
        let lines = LineIndex::new(&a.text, self.encoding);
        (lines.offset(range.start), lines.offset(range.end))
    }

    fn write_and_view(
        &self,
        uri: &Url,
        a: &Analysis,
        id: &str,
        kind: &str,
        fields: Map<String, Value>,
    ) -> Result<Value, Failure> {
        self.write_event(a, id, kind, fields)?;
        self.view_of(uri, id)
    }

    fn m_annotations(&mut self, params: &Value) -> Result<Value, Failure> {
        let uri = uri_param(&params["textDocument"], "uri")?;
        let include_closed = params["includeClosed"].as_bool().unwrap_or(false);
        let include_deleted = params["includeDeleted"].as_bool().unwrap_or(false);
        if let Some(doc) = self.open.get_mut(&uri) {
            doc.include_closed = include_closed;
            doc.include_deleted = include_deleted;
        }
        // An explicit request always reflects storage as it is now.
        self.invalidate();
        let a = self.analyze_uri(&uri)?;
        Ok(json!({
            "annotations": views::views(&a, self.encoding, include_closed, include_deleted),
            "document": views::document_info(&a),
        }))
    }

    /// Creates an annotation, first offering to create a workspace if the
    /// document isn't in one (§6.2).
    fn m_create_or_init(&mut self, command: RequestId, params: Value) {
        let uri = match uri_param(&params["textDocument"], "uri") {
            Ok(uri) => uri,
            Err(e) => return self.reply(command, Err(e)),
        };
        let path = uri.to_file_path().ok();
        if self.analyze(&uri).is_some() || path.as_ref().is_none_or(|p| Workspace::find(p).is_some()) {
            let result = self.m_create(&params);
            self.reply(command, result);
            return self.refresh_all();
        }
        let path = path.expect("checked above");
        let root = self
            .roots
            .iter()
            .filter(|r| path.starts_with(r))
            .max_by_key(|r| r.components().count())
            .cloned()
            .or_else(|| path.parent().map(|p| p.to_path_buf()))
            .unwrap_or_default();
        let message = format!("{} is not in an annox workspace. Create one in {}?", path.display(), root.display());
        let request = json!({
            "type": 3,
            "message": message,
            "actions": [{ "title": CREATE_WORKSPACE }, { "title": "Cancel" }],
        });
        self.next_id += 1;
        let id = RequestId::from(format!("annox-init-{}", self.next_id));
        self.pending_init.insert(id.clone(), PendingInit { command, params, root });
        self.send(lsp_server::Request::new(id, "window/showMessageRequest".into(), request));
    }

    /// Continues a create after the user answered the workspace prompt.
    pub(crate) fn finish_init(&mut self, init: PendingInit, resp: lsp_server::Response) {
        let chosen = resp.response_result.ok().and_then(|v| v["title"].as_str().map(str::to_owned));
        if chosen.as_deref() != Some(CREATE_WORKSPACE) {
            return self.reply(init.command, fail(NO_WORKSPACE, "no annox workspace was created"));
        }
        if let Err(e) = Workspace::init(&init.root) {
            return self.reply(init.command, Err(io_failure(e)));
        }
        self.invalidate();
        let result = self.m_create(&init.params);
        self.reply(init.command, result);
        self.refresh_all();
    }

    fn m_create(&self, params: &Value) -> Result<Value, Failure> {
        let uri = uri_param(&params["textDocument"], "uri")?;
        let a = self.analyze_uri(&uri)?;
        let kind = str_param(params, "kind")?;
        let replacement = params["replacement"].as_str();
        match (kind, replacement) {
            ("comment", _) | ("suggestion", Some(_)) => {}
            ("suggestion", None) => return fail(INVALID_PARAMS, "a suggestion needs a replacement"),
            _ => return fail(INVALID_PARAMS, format!("cannot create a {kind}")),
        }
        let (start, end) = self.offsets(&a, range_param(params)?);
        if kind == "suggestion" && replacement == Some(a.text.slice(start, end).as_str()) {
            return fail(INVALID_PARAMS, "the replacement equals the current text");
        }
        let new = NewAnnotation {
            kind,
            start,
            end,
            body: params["body"].as_str(),
            label: params["label"].as_str(),
            replacement,
            local: params["local"].as_bool().unwrap_or(false),
        };
        let event = ops::create_annotation(&a.ws, &a.index, &a.rel, &a.text, &new, &self.author(&a.ws));
        self.invalidate();
        let event = event.map_err(io_failure)?;
        self.view_of(&uri, &event.id)
    }

    fn m_reply(&self, params: &Value) -> Result<Value, Failure> {
        let parent = str_param(params, "parent")?;
        let body = str_param(params, "body")?;
        if body.is_empty() {
            return fail(INVALID_PARAMS, "a reply needs a body");
        }
        let (uri, a) = self.locate(parent)?;
        if a.item(parent).is_none() {
            return fail(INVALID_OPERATION, "replies must be to a comment or suggestion");
        }
        let local = params["local"].as_bool().unwrap_or(false);
        let event = ops::create_reply(&a.ws, &a.index, parent, body, local, &self.author(&a.ws));
        self.invalidate();
        let event = event.map_err(io_failure)?;
        self.view_of(&uri, &event.id)
    }

    fn m_publish(&self, params: &Value) -> Result<Value, Failure> {
        let ids: Vec<String> = serde_json::from_value(params["annotations"].clone())
            .map_err(|e| (INVALID_PARAMS, format!("annotations: {e}")))?;
        let mut published = Vec::new();
        for id in &ids {
            let (uri, a) = self.locate(id)?;
            let moved = ops::publish(&a.ws, &a.index, std::slice::from_ref(id), &self.author(&a.ws));
            self.invalidate();
            let moved = moved.map_err(io_failure)?;
            if !moved.is_empty() {
                published.push(self.view_of(&uri, id)?);
            }
        }
        Ok(json!(published))
    }

    fn m_edit(&self, params: &Value) -> Result<Value, Failure> {
        let id = str_param(params, "annotation")?;
        let (uri, a) = self.locate(id)?;
        let is_reply = a.reply(id).is_some();
        let mut fields = Map::new();
        for name in ["body", "label"] {
            let Some(value) = params.get(name) else { continue };
            if !(value.is_null() || value.is_string()) {
                return fail(INVALID_PARAMS, format!("{name} must be a string or null"));
            }
            if is_reply && (name == "label" || value.as_str().is_none_or(str::is_empty)) {
                return fail(INVALID_OPERATION, "replies have a non-empty body and no label");
            }
            fields.insert(name.into(), value.clone());
        }
        if fields.is_empty() {
            return fail(INVALID_PARAMS, "nothing to edit");
        }
        self.write_and_view(&uri, &a, id, "edit", fields)
    }

    fn m_set_status(&self, params: &Value) -> Result<Value, Failure> {
        let id = str_param(params, "annotation")?;
        let status = str_param(params, "status")?;
        let (uri, a) = self.locate(id)?;
        let item = a.item(id).ok_or((INVALID_OPERATION, "replies have no status".to_owned()))?;
        let allowed: &[&str] = match item.kind() {
            "comment" => &["open", "resolved"],
            _ => &["open", "rejected", "withdrawn"],
        };
        if !allowed.contains(&status) {
            let hint = if status == "accepted" { " (use annox/accept)" } else { "" };
            return fail(INVALID_OPERATION, format!("cannot set status {status} on a {}{hint}", item.kind()));
        }
        self.write_and_view(&uri, &a, id, "status", Map::from_iter([("status".into(), json!(status))]))
    }

    fn m_retarget(&self, params: &Value) -> Result<Value, Failure> {
        let id = str_param(params, "annotation")?;
        let replacement = str_param(params, "replacement")?;
        let (uri, a) = self.locate(id)?;
        if a.item(id).is_none_or(|i| i.kind() != "suggestion") {
            return fail(INVALID_OPERATION, "only suggestions can be re-targeted");
        }
        let (start, end) = self.offsets(&a, range_param(params)?);
        let fields = Map::from_iter([
            ("target".into(), json!(anchor::create(&a.text, start, end, &a.rel))),
            ("edit".into(), json!({ "replacement": replacement })),
        ]);
        self.write_and_view(&uri, &a, id, "retarget", fields)
    }

    fn m_reattach(&self, params: &Value) -> Result<Value, Failure> {
        let id = str_param(params, "annotation")?;
        let (uri, a) = self.locate(id)?;
        if a.item(id).is_none_or(|i| i.kind() != "comment") {
            return fail(INVALID_OPERATION, "only comments can be re-attached; use annox/retarget for suggestions");
        }
        let (start, end) = self.offsets(&a, range_param(params)?);
        let fields = Map::from_iter([("target".into(), json!(anchor::create(&a.text, start, end, &a.rel)))]);
        self.write_and_view(&uri, &a, id, "reanchor", fields)
    }

    fn m_simple(&self, params: &Value, kind: &str) -> Result<Value, Failure> {
        let id = str_param(params, "annotation")?;
        let (uri, a) = self.locate(id)?;
        self.write_and_view(&uri, &a, id, kind, Map::new())
    }

    fn m_move_document(&self, params: &Value) -> Result<Value, Failure> {
        let (from, to) = (uri_param(params, "from")?, uri_param(params, "to")?);
        let (Ok(from), Ok(to)) = (from.to_file_path(), to.to_file_path()) else {
            return fail(INVALID_PARAMS, "from and to must be file URIs");
        };
        let ws = Workspace::find(&to).ok_or((NO_WORKSPACE, "not in an annox workspace".to_owned()))?;
        let (Some(from), Some(to)) = (ws.relative(&from), ws.relative(&to)) else {
            return fail(INVALID_PARAMS, "both paths must be inside the workspace");
        };
        let moved = ops::move_document(&ws, &Index::read(&ws), &from, &to, &self.author(&ws));
        self.invalidate();
        let moved = moved.map_err(io_failure)?;
        Ok(json!({ "moved": moved }))
    }

    fn m_history(&self, params: &Value) -> Result<Value, Failure> {
        let id = str_param(params, "annotation")?;
        let (_, a) = self.locate(id)?;
        Ok(json!(replay::ordered_events(&a.loaded.events, id)))
    }

    fn m_accept(&mut self, command: RequestId, params: &Value) {
        let located = str_param(params, "annotation").and_then(|id| self.locate(id).map(|l| (id.to_owned(), l)));
        match located {
            Ok((id, (uri, a))) => self.accept(command, uri, &a, &id, true),
            Err(e) => self.reply(command, Err(e)),
        }
    }

    /// Bulk accept (§6.6.2): skips stale and overlapping suggestions, and
    /// step-3 and step-5 ones unless `confirmed`, then applies the rest as
    /// one edit.
    fn m_accept_all(&mut self, command: RequestId, params: &Value) {
        let ids: Vec<String> = match serde_json::from_value(params["annotations"].clone()) {
            Ok(ids) => ids,
            Err(e) => return self.reply(command, fail(INVALID_PARAMS, format!("annotations: {e}"))),
        };
        let confirmed = params["confirmed"].as_bool().unwrap_or(false);
        let mut target: Option<(Url, std::rc::Rc<Analysis>)> = None;
        let mut results = Vec::new();
        let mut chosen: Vec<String> = Vec::new();
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        for id in &ids {
            let Some((uri, a)) = self.find(id) else {
                results.push(views::result_error(id, UNKNOWN_ANNOTATION, "unknown annotation"));
                continue;
            };
            if target.as_ref().is_some_and(|(u, _)| *u != uri) {
                results.push(views::result_error(id, INVALID_OPERATION, "all suggestions must be in one document"));
                continue;
            }
            let (_, a) = target.get_or_insert((uri, a));
            let Some(item) = a.item(id).filter(|i| i.kind() == "suggestion" && i.is_open()) else {
                results.push(views::result_error(id, INVALID_OPERATION, "not an open suggestion"));
                continue;
            };
            let replacement = item.state["edit"]["replacement"].as_str().unwrap_or_default().to_owned();
            let error = match suggestion::apply(&a.text, &item.target, &replacement) {
                Err(_) => Some((STALE_SUGGESTION, "the suggestion is stale")),
                Ok(applied) if applied.resolution.step >= 3 && !confirmed => {
                    Some((NEEDS_REVIEW, "relocated by partial context; accept individually"))
                }
                Ok(applied) if edits.iter().any(|(s, e, _)| overlaps((*s, *e), (applied.start, applied.end))) => {
                    Some((OVERLAP, "overlaps a suggestion already in this batch"))
                }
                Ok(applied) => {
                    edits.push((applied.start, applied.end, replacement));
                    chosen.push(id.clone());
                    None
                }
            };
            results.push(match error {
                Some((code, msg)) => views::result_error(id, code, msg),
                None => json!({ "annotation": id }),
            });
        }
        match target {
            Some((uri, _)) if !chosen.is_empty() => {
                self.apply_edit(command, uri, edits, Work::AcceptAll { chosen, results }, false);
            }
            _ => self.reply(command, Ok(json!({ "results": results }))),
        }
    }

    fn m_resolve_conflict(&mut self, command: RequestId, params: &Value) {
        let result = self.resolve_conflict(command.clone(), params);
        if let Some(result) = result {
            self.reply(command, result);
            self.refresh_all();
        }
    }

    /// Returns `None` if the reply is deferred until an edit is applied.
    fn resolve_conflict(&mut self, command: RequestId, params: &Value) -> Option<Result<Value, Failure>> {
        let id = match str_param(params, "annotation") {
            Ok(id) => id.to_owned(),
            Err(e) => return Some(Err(e)),
        };
        let (field, value) = (params["field"].as_str().unwrap_or_default(), params["value"].clone());
        let (uri, a) = match self.locate(&id) {
            Ok(l) => l,
            Err(e) => return Some(Err(e)),
        };
        let state = a.item(&id).map(|i| i.state.clone()).or_else(|| a.reply(&id).cloned())?;
        let Some(heads) = state["conflicts"][field].as_array().cloned() else {
            return Some(fail(NOT_CONFLICTED, format!("{field} is not conflicted")));
        };
        let head_events: Vec<_> =
            heads.iter().filter_map(|h| a.index.events.get(h.as_str()?)).map(|(e, _)| e.clone()).collect();
        let (kind, fields): (&str, Map<String, Value>) = match field {
            "body" | "label" => ("edit", Map::from_iter([(field.to_owned(), value.clone())])),
            "deleted" => match value.as_bool() {
                Some(true) => ("delete", Map::new()),
                Some(false) => ("restore", Map::new()),
                None => return Some(fail(INVALID_PARAMS, "value must be a boolean")),
            },
            "target" | "replacement" => {
                let target = if field == "target" { value.clone() } else { state["target"].clone() };
                if serde_json::from_value::<Anchor>(target.clone()).is_err() {
                    return Some(fail(INVALID_PARAMS, "value must be an anchor"));
                }
                if state["kind"] == "comment" {
                    ("reanchor", Map::from_iter([("target".into(), target)]))
                } else {
                    let replacement =
                        if field == "replacement" { value.clone() } else { state["edit"]["replacement"].clone() };
                    (
                        "retarget",
                        Map::from_iter([
                            ("target".into(), target),
                            ("edit".into(), json!({ "replacement": replacement })),
                        ]),
                    )
                }
            }
            "status" => {
                let Some(status) = value.as_str() else { return Some(fail(INVALID_PARAMS, "value must be a status")) };
                let accepted = head_events.iter().find(|e| e.field("status") == Some(&json!("accepted")));
                let mut fields = Map::from_iter([("status".into(), json!(status))]);
                if status == "accepted" {
                    let Some(version) = accepted.and_then(|e| e.field("appliedVersion")) else {
                        return Some(fail(INVALID_OPERATION, "accepted is not one of the competing values"));
                    };
                    fields.insert("appliedVersion".into(), version.clone());
                } else if accepted.is_some() && params["revert"].as_bool().unwrap_or(false) {
                    // Revert the applied edit first (§4.3.3).
                    let item = a.item(&id)?;
                    let replacement = item.state["edit"]["replacement"].as_str().unwrap_or_default();
                    if let Some((c, d)) = suggestion::applied_text_search(&a.text, &item.target, replacement) {
                        let original = item.target.selectors.quote.exact.clone();
                        let work = Work::Revert { annotation: id.clone(), status: status.to_owned() };
                        self.apply_edit(command, uri, vec![(c, d, original)], work, true);
                        return None;
                    }
                }
                ("status", fields)
            }
            _ => return Some(fail(INVALID_PARAMS, format!("unknown field {field}"))),
        };
        Some(self.write_and_view(&uri, &a, &id, kind, fields))
    }
}
