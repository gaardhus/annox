//! A Model Context Protocol server over stdio (`annox mcp`), for agents that
//! can't run shell commands. Each tool is one command of [`crate::cli`].

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

/// Protocol versions this server speaks, newest first.
const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Annotations (comments and suggested edits) on files in an annox workspace. \
Target text by quoting it exactly; if a quote occurs more than once, quote more of it or pass `occurrence`. \
To propose changes to someone's prose, prefer `suggest` over editing the file, so they can review each change. \
Make one suggestion per change, quoting only the words that change, so they can accept some and reject others; \
suggest a whole sentence or paragraph only when rewriting most of it. \
Read review comments with `list_annotations`, and answer with `reply`, `suggest`, and `set_status`. \
To pick up what others changed since your last session, list with `unread`, and `mark_read` each thread you dealt with. \
Bodies are Markdown: write ≈ or \"about\" rather than `~`, since two single tildes strike through the text between them.";

const CHANNEL_INSTRUCTIONS: &str = " \
If an `annox` skill is available, load it before handling the first thread. \
Threads that others changed and that are addressed to you arrive as <channel source=\"annox\" annotation_id=\"…\" path=\"…\">, \
with the thread as list_annotations prints it. They're from the people reviewing the files: act on each one as they ask. \
Answer with `reply`; propose changes to their prose with `suggest`, not by editing the file; then `mark_read` the thread. \
If a thread needs nothing from you, just `mark_read` it.";

pub struct Config {
    /// Directory commands run in: file paths are relative to it.
    pub root: PathBuf,
    /// Author id and name for write commands, when set.
    pub author: Option<String>,
    pub name: Option<String>,
    /// When set, push the threads this watch reports to the client as
    /// Claude Code channel events (`notifications/claude/channel`).
    pub channel: Option<crate::cli::Watch>,
}

/// A tool: its name, the CLI command it runs, description, and parameters as
/// `(name, type, required, description)`. `positional` parameters come first
/// in the command line, in order.
struct Tool {
    name: &'static str,
    command: &'static str,
    description: &'static str,
    positional: &'static [&'static str],
    params: &'static [(&'static str, &'static str, bool, &'static str)],
    read_only: bool,
}

const ID: (&str, &str, bool, &str) = ("id", "string", true, "Annotation id, from list_annotations.");
const FILE: (&str, &str, bool, &str) = ("file", "string", true, "Path of the file, relative to the workspace.");
const QUOTE: (&str, &str, bool, &str) = ("quote", "string", true, "Text to target, exactly as it appears in the file.");
const NEW_QUOTE: (&str, &str, bool, &str) = (
    "quote",
    "string",
    false,
    "Text to target, exactly as it appears in the file. Required unless `suggested` is set.",
);
const SUGGESTED: (&str, &str, bool, &str) = (
    "suggested",
    "boolean",
    false,
    "Target the `suggested` location list_annotations shows for an orphaned annotation, instead of a quote. Check that it's right first.",
);
const OCCURRENCE: (&str, &str, bool, &str) = (
    "occurrence",
    "integer",
    false,
    "Which occurrence of the quote to target (1-based), when it occurs more than once.",
);
const LABEL: (&str, &str, bool, &str) = ("label", "string", false, "A short label, such as a category.");
const LOCAL: (&str, &str, bool, &str) =
    ("local", "boolean", false, "Keep the annotation private to this machine, as a draft.");

const TOOLS: &[Tool] = &[
    Tool {
        name: "list_annotations",
        command: "list",
        description: "List annotations with their current line and quoted text, replies, and for suggestions the replacement and whether it can still be applied. Without `file`, lists every document in the workspace.",
        positional: &["file"],
        params: &[
            ("file", "string", false, "Only this file."),
            ("all", "boolean", false, "Include resolved, accepted, rejected, withdrawn and deleted annotations."),
            ("closed", "boolean", false, "Include resolved, accepted, rejected and withdrawn annotations, but not deleted ones."),
            ("kind", "string", false, "Only `comment` or only `suggestion`."),
            ("status", "string", false, "Only these statuses, comma-separated: open, resolved, accepted, rejected, withdrawn."),
            ("author", "string", false, "Only annotations by this author id."),
            ("not_author", "string", false, "Skip annotations by this author id."),
            ("mine", "boolean", false, "Only your own annotations."),
            ("others", "boolean", false, "Skip your own annotations."),
            ("broken", "boolean", false, "Only open annotations that are orphaned or can't be applied, to fix with retarget or reattach. An orphaned one may have a `suggested` location: where its text probably went, with the share of its words found there."),
            ("unread", "boolean", false, "Only threads others changed since you last marked them read with mark_read: new annotations, replies, edits and status changes, closed threads included. Each annotation and reply has `unread`."),
            ("quote", "string", false, "Only annotations on this text, exactly as it appears in `file` (required), including resolved, accepted, rejected and withdrawn ones unless `status` is given: everything said about a passage. An accepted suggestion is on the text it put in."),
            OCCURRENCE,
        ],
        read_only: true,
    },
    Tool {
        name: "report",
        command: "report",
        description: "Count each document's annotations: open comments and suggestions by status, and how many are orphaned, stale (can't be applied) or conflicted. A cheap overview before listing. Without `file`, covers every document in the workspace.",
        positional: &["file"],
        params: &[("file", "string", false, "Only this file.")],
        read_only: true,
    },
    Tool {
        name: "show_annotation",
        command: "show",
        description: "Show one annotation and its thread, in the same form as list_annotations. Given a reply's id, shows its thread.",
        positional: &["id"],
        params: &[ID],
        read_only: true,
    },
    Tool {
        name: "history",
        command: "history",
        description: "List the events of an annotation's thread, oldest first: who created, edited, resolved, accepted, rejected, withdrew, deleted or restored the annotation or its replies, and when. Given a reply's id, shows its thread.",
        positional: &["id"],
        params: &[ID],
        read_only: true,
    },
    Tool {
        name: "comment",
        command: "comment",
        description: "Comment on a passage of a file.",
        positional: &["file"],
        params: &[FILE, QUOTE, ("body", "string", true, "The comment, in Markdown."), OCCURRENCE, LABEL, LOCAL],
        read_only: false,
    },
    Tool {
        name: "highlight",
        command: "highlight",
        description: "Highlight a passage of a file: a comment with no body.",
        positional: &["file"],
        params: &[FILE, QUOTE, OCCURRENCE, LABEL, LOCAL],
        read_only: false,
    },
    Tool {
        name: "suggest",
        command: "suggest",
        description: "Suggest replacing a passage of a file. The file is not changed until someone accepts the suggestion.",
        positional: &["file"],
        params: &[
            FILE,
            QUOTE,
            ("replacement", "string", true, "Text to replace the quote with. Empty to suggest deleting it."),
            ("body", "string", false, "Why, in Markdown."),
            OCCURRENCE,
            LABEL,
            LOCAL,
        ],
        read_only: false,
    },
    Tool {
        name: "reply",
        command: "reply",
        description: "Reply to a comment or suggestion.",
        positional: &["id"],
        params: &[ID, ("body", "string", true, "The reply, in Markdown.")],
        read_only: false,
    },
    Tool {
        name: "edit",
        command: "edit",
        description: "Change the body or label of an annotation or reply.",
        positional: &["id"],
        params: &[ID, ("body", "string", false, "New body."), ("label", "string", false, "New label; empty removes it.")],
        read_only: false,
    },
    Tool {
        name: "set_status",
        command: "status",
        description: "Set the status of a comment (open, resolved) or suggestion (open, rejected, withdrawn). Use `accept` to accept a suggestion.",
        positional: &["id", "status"],
        params: &[ID, ("status", "string", true, "open, resolved, rejected, or withdrawn.")],
        read_only: false,
    },
    Tool {
        name: "accept",
        command: "accept",
        description: "Apply a suggestion to its file and mark it accepted. Only do this when the user asks.",
        positional: &["id"],
        params: &[
            ID,
            ("confirmed", "boolean", false, "Accept even though the suggestion was relocated, after checking its new location."),
        ],
        read_only: false,
    },
    Tool {
        name: "revert",
        command: "revert",
        description: "Undo an accepted suggestion: create a new suggestion that puts the original text back, linked to it by `reverts`, or use the open one that already does. Fails if it was already reverted, or if the accepted text was changed since. Only do this when the user asks.",
        positional: &["id"],
        params: &[
            ID,
            ("accept", "boolean", false, "Also accept the new suggestion, applying the revert to the file."),
            ("body", "string", false, "Why it's being reverted, in Markdown."),
        ],
        read_only: false,
    },
    Tool {
        name: "retarget",
        command: "retarget",
        description: "Point an open suggestion at new text, with a reviewed replacement. Use it when a suggestion is orphaned or no longer applicable.",
        positional: &["id"],
        params: &[ID, NEW_QUOTE, SUGGESTED, ("replacement", "string", true, "Replacement for the new quote."), OCCURRENCE],
        read_only: false,
    },
    Tool {
        name: "reattach",
        command: "reattach",
        description: "Point an open comment at new text, when it is orphaned or attached to the wrong passage.",
        positional: &["id"],
        params: &[ID, NEW_QUOTE, SUGGESTED, OCCURRENCE],
        read_only: false,
    },
    Tool {
        name: "mark_read",
        command: "read",
        description: "Mark a thread read, or every thread, so list_annotations with `unread` stops listing it until someone changes it again. Mark only threads you have dealt with. Writing to a thread marks it read too.",
        positional: &["id"],
        params: &[
            ("id", "string", false, "Annotation id, from list_annotations. A reply's marks its thread."),
            ("all", "boolean", false, "Mark every thread read."),
        ],
        read_only: false,
    },
    Tool {
        name: "delete",
        command: "delete",
        description: "Hide an annotation or reply. Its history is kept, and `restore` undoes it.",
        positional: &["id"],
        params: &[ID],
        read_only: false,
    },
    Tool {
        name: "restore",
        command: "restore",
        description: "Undo the deletion of an annotation or reply.",
        positional: &["id"],
        params: &[ID],
        read_only: false,
    },
];

/// CLI option names that differ from tool parameter names.
fn option_name(param: &str) -> &str {
    match param {
        "replacement" => "replace",
        "not_author" => "not-author",
        other => other,
    }
}

fn describe(tool: &Tool) -> Value {
    let properties: serde_json::Map<String, Value> = tool
        .params
        .iter()
        .map(|(name, ty, _, description)| (name.to_string(), json!({ "type": ty, "description": description })))
        .collect();
    let required: Vec<&str> = tool.params.iter().filter(|p| p.2).map(|p| p.0).collect();
    json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
        // Accepting rewrites the user's file; everything else only adds events.
        "annotations": { "readOnlyHint": tool.read_only, "destructiveHint": tool.name == "accept" },
    })
}

/// Turns tool arguments into a command line for [`crate::cli::run`].
fn command_line(tool: &Tool, arguments: &Value, config: &Config) -> Result<Vec<String>, String> {
    let as_string = |v: &Value| match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(format!("expected a string, got {other}")),
    };
    let mut args = vec![tool.command.to_owned()];
    for name in tool.positional {
        if let Some(v) = arguments.get(name).filter(|v| !v.is_null()) {
            args.push(as_string(v).map_err(|e| format!("{name}: {e}"))?);
        }
    }
    for (name, ty, required, _) in tool.params {
        let value = arguments.get(name).filter(|v| !v.is_null());
        // `mine` and `others` mean the server's identity, when it has one.
        if let (Some(author), "mine" | "others", Some(true)) = (&config.author, *name, value.and_then(Value::as_bool)) {
            args.extend([format!("--{}", if *name == "mine" { "author" } else { "not-author" }), author.clone()]);
            continue;
        }
        match value {
            None if *required => return Err(format!("missing argument {name}")),
            None => {}
            Some(_) if tool.positional.contains(name) => {}
            Some(v) if *ty == "boolean" => {
                if v.as_bool().ok_or_else(|| format!("{name}: expected a boolean"))? {
                    args.push(format!("--{}", option_name(name)));
                }
            }
            Some(v) => {
                args.push(format!("--{}", option_name(name)));
                args.push(as_string(v).map_err(|e| format!("{name}: {e}"))?);
            }
        }
    }
    if let Some(unknown) =
        arguments.as_object().into_iter().flatten().map(|(k, _)| k).find(|k| !tool.params.iter().any(|p| p.0 == *k))
    {
        return Err(format!("unknown argument {unknown}"));
    }
    // Whose read state `unread` uses.
    if let (Some(author), "list") = (&config.author, tool.command) {
        args.extend(["--as".into(), author.clone()]);
    }
    if !tool.read_only {
        if let Some(author) = &config.author {
            args.extend(["--author".into(), author.clone()]);
        }
        if let Some(name) = &config.name {
            args.extend(["--name".into(), name.clone()]);
        }
    }
    Ok(args)
}

fn call_tool(params: &Value, config: &Config) -> Result<Value, (i64, String)> {
    let name = params["name"].as_str().ok_or((-32602, "missing tool name".to_owned()))?;
    let tool = TOOLS.iter().find(|t| t.name == name).ok_or((-32602, format!("unknown tool {name}")))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    // Errors from the command go back to the model as tool results, so it
    // can correct itself (e.g. quote more text).
    let result = command_line(tool, &arguments, config)
        .and_then(|args| crate::cli::run(&args, &config.root).map_err(|e| format!("{e:#}")));
    Ok(match result {
        Ok(output) => json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&output).unwrap_or_default() }],
            "isError": false,
        }),
        Err(message) => json!({ "content": [{ "type": "text", "text": message }], "isError": true }),
    })
}

fn handle(method: &str, params: &Value, config: &Config) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => {
            let requested = params["protocolVersion"].as_str().unwrap_or_default();
            let version = VERSIONS.iter().find(|v| **v == requested).unwrap_or(&VERSIONS[0]);
            let mut capabilities = json!({ "tools": {} });
            let mut instructions = INSTRUCTIONS.to_owned();
            if config.channel.is_some() {
                capabilities["experimental"] = json!({ "claude/channel": {} });
                instructions.push_str(CHANNEL_INSTRUCTIONS);
            }
            Ok(json!({
                "protocolVersion": version,
                "capabilities": capabilities,
                "serverInfo": { "name": "annox", "version": env!("CARGO_PKG_VERSION") },
                "instructions": instructions,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": TOOLS.iter().map(describe).collect::<Vec<_>>() })),
        "tools/call" => call_tool(params, config),
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}

/// The channel event for a thread a watch reported.
fn channel_event(thread: &Value) -> Value {
    let meta: serde_json::Map<String, Value> = [("annotation_id", "id"), ("path", "path"), ("kind", "kind")]
        .into_iter()
        .filter_map(|(key, field)| Some((key.to_owned(), json!(thread[field].as_str()?))))
        .chain(thread["line"].as_u64().map(|l| ("line".to_owned(), json!(l.to_string()))))
        .collect();
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/claude/channel",
        "params": { "content": serde_json::to_string_pretty(thread).unwrap_or_default(), "meta": meta },
    })
}

fn send(output: &Mutex<impl Write>, message: &Value) -> std::io::Result<()> {
    let mut output = output.lock().unwrap_or_else(|e| e.into_inner());
    writeln!(output, "{message}")?;
    output.flush()
}

/// Serves newline-delimited JSON-RPC messages from `input` until it closes.
/// With a channel, it also watches the workspace once the client has
/// initialized, and pushes the threads the watch reports.
pub fn serve(input: impl BufRead, output: impl Write + Send, config: &Config) -> std::io::Result<()> {
    let output = Mutex::new(output);
    let initialized = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        if let Some(watch) = &config.channel {
            let (output, initialized, done) = (&output, &initialized, &done);
            scope.spawn(move || {
                while !initialized.load(Ordering::Relaxed) {
                    if done.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                let emit = |thread: Value| send(output, &channel_event(&thread)).map_err(anyhow::Error::from);
                let watched = crate::cli::Watcher::new(watch.clone(), &config.root);
                if let Err(e) = watched.and_then(|watcher| crate::cli::watch(watcher, done, emit)) {
                    eprintln!("annox mcp: channel stopped: {e:#}");
                }
            });
        }
        let result = respond(input, &output, &initialized, config);
        done.store(true, Ordering::Relaxed);
        result
    })
}

fn respond(
    input: impl BufRead,
    output: &Mutex<impl Write>,
    initialized: &AtomicBool,
    config: &Config,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Err(e) => json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } }),
            // Notifications get no response.
            Ok(msg) if msg.get("id").is_none() => {
                if msg["method"] == "notifications/initialized" {
                    initialized.store(true, Ordering::Relaxed);
                }
                continue;
            }
            // Responses to requests we never send.
            Ok(msg) if msg.get("method").is_none() => continue,
            Ok(msg) => {
                let method = msg["method"].as_str().unwrap_or_default();
                let params = msg.get("params").cloned().unwrap_or(json!({}));
                match handle(method, &params, config) {
                    Ok(result) => json!({ "jsonrpc": "2.0", "id": msg["id"], "result": result }),
                    Err((code, message)) => {
                        json!({ "jsonrpc": "2.0", "id": msg["id"], "error": { "code": code, "message": message } })
                    }
                }
            }
        };
        send(output, &response)?;
    }
    Ok(())
}
