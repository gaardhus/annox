//! A Model Context Protocol server over stdio (`annox mcp`), for agents that
//! can't run shell commands. Each tool is one command of [`crate::cli`].

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{json, Value};

/// Protocol versions this server speaks, newest first.
const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Annotations (comments and suggested edits) on files in an annox workspace. \
Target text by quoting it exactly; if a quote occurs more than once, quote more of it or pass `occurrence`. \
To propose changes to someone's prose, prefer `suggest` over editing the file, so they can review each change. \
Read review comments with `list_annotations`, and answer with `reply`, `suggest`, and `set_status`.";

pub struct Config {
    /// Directory commands run in: file paths are relative to it.
    pub root: PathBuf,
    /// Author id and name for write commands, when set.
    pub author: Option<String>,
    pub name: Option<String>,
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
        ],
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
        name: "retarget",
        command: "retarget",
        description: "Point an open suggestion at new text, with a reviewed replacement. Use it when a suggestion is orphaned or no longer applicable.",
        positional: &["id"],
        params: &[ID, QUOTE, ("replacement", "string", true, "Replacement for the new quote."), OCCURRENCE],
        read_only: false,
    },
    Tool {
        name: "reattach",
        command: "reattach",
        description: "Point an open comment at new text, when it is orphaned or attached to the wrong passage.",
        positional: &["id"],
        params: &[ID, QUOTE, OCCURRENCE],
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
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "annox", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": TOOLS.iter().map(describe).collect::<Vec<_>>() })),
        "tools/call" => call_tool(params, config),
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}

/// Serves newline-delimited JSON-RPC messages from `input` until it closes.
pub fn serve(input: impl BufRead, mut output: impl Write, config: &Config) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Err(e) => json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } }),
            // Notifications get no response.
            Ok(msg) if msg.get("id").is_none() => continue,
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
        writeln!(output, "{response}")?;
        output.flush()?;
    }
    Ok(())
}
