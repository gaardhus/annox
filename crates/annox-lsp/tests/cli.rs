//! Tests of the command-line interface, run in-process.

use std::path::Path;

use annox_lsp::{cli, render};
use serde_json::Value;

const DOC: &str = "In Section 3, we prove that teh bound is tight.\nThe bound is tight.\n";

fn setup(doc: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("paper.md"), doc).unwrap();
    annox(dir.path(), &["init"]);
    dir
}

fn try_annox(cwd: &Path, args: &[&str]) -> anyhow::Result<Value> {
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    if !matches!(
        args[0].as_str(),
        "init" | "list" | "show" | "history" | "log" | "report" | "commit" | "export-pdf" | "text"
    ) {
        args.extend(["--author".into(), "urn:test:agent".into(), "--name".into(), "Agent".into()]);
    }
    cli::run(&args, cwd)
}

fn annox(cwd: &Path, args: &[&str]) -> Value {
    try_annox(cwd, args).unwrap()
}

fn error(cwd: &Path, args: &[&str]) -> String {
    format!("{:#}", try_annox(cwd, args).unwrap_err())
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_owned()
}

#[test]
fn requires_a_workspace() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("paper.md"), DOC).unwrap();
    let err = error(dir.path(), &["comment", "paper.md", "--quote", "teh", "--body", "x"]);
    assert!(err.contains("annox init"), "{err}");
}

#[test]
fn ambiguous_quotes_need_an_occurrence() {
    let dir = setup(DOC);
    let err = error(dir.path(), &["comment", "paper.md", "--quote", "bound is tight", "--body", "x"]);
    assert!(err.contains("occurs 2 times (lines 1, 2)"), "{err}");
    let err = error(dir.path(), &["comment", "paper.md", "--quote", "not there", "--body", "x"]);
    assert!(err.contains("not found"), "{err}");

    let created =
        annox(dir.path(), &["comment", "paper.md", "--quote", "bound is tight", "--occurrence", "2", "--body", "x"]);
    assert_eq!(created["line"], 2);
    let list = annox(dir.path(), &["list", "paper.md"]);
    assert_eq!(list[0]["line"], 2);
    assert_eq!(list[0]["resolution"], "exact");
    assert_eq!(list[0]["author"]["id"], "urn:test:agent");
}

#[test]
fn highlights_are_comments_without_a_body() {
    let dir = setup(DOC);
    let h = id(&annox(dir.path(), &["highlight", "paper.md", "--quote", "bound is tight", "--occurrence", "2"]));
    let shown = annox(dir.path(), &["show", &h]);
    assert_eq!((&shown["kind"], &shown["body"]), (&"comment".into(), &Value::Null));
    annox(dir.path(), &["edit", &h, "--body", "Why?"]);
    assert_eq!(annox(dir.path(), &["show", &h])["body"], "Why?", "adding a body turns it into a comment");
}

#[test]
fn threads_and_statuses() {
    let dir = setup(DOC);
    let c = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "Section 3", "--body", "Which one?"]));
    let r = id(&annox(dir.path(), &["reply", &c, "--body", "The third."]));
    assert!(error(dir.path(), &["reply", &r, "--body", "nested"]).contains("reply to its parent"));

    let list = annox(dir.path(), &["list"]);
    assert_eq!(list[0]["replies"][0]["body"], "The third.");

    assert!(error(dir.path(), &["status", &c, "rejected"]).contains("open, resolved"));
    annox(dir.path(), &["status", &c, "resolved"]);
    assert_eq!(annox(dir.path(), &["list"]), serde_json::json!([]));
    assert_eq!(annox(dir.path(), &["list", "--all"])[0]["status"], "resolved");
    assert_eq!(annox(dir.path(), &["list", "--closed"])[0]["status"], "resolved");

    annox(dir.path(), &["edit", &c, "--body", "Which section?", "--label", "question"]);
    let all = annox(dir.path(), &["list", "--all"]);
    assert_eq!((&all[0]["body"], &all[0]["label"]), (&"Which section?".into(), &"question".into()));
    annox(dir.path(), &["edit", &c, "--label", ""]);
    assert_eq!(annox(dir.path(), &["list", "--all"])[0]["label"], Value::Null);

    annox(dir.path(), &["delete", &c]);
    assert_eq!(annox(dir.path(), &["list", "--all"])[0]["deleted"], true);
    assert_eq!(annox(dir.path(), &["list", "--closed"]), serde_json::json!([]), "--closed leaves out deleted ones");
    assert!(error(dir.path(), &["list", "--all", "--closed"]).contains("cannot be used with"));
}

#[test]
fn revert_undoes_an_accepted_suggestion() {
    let dir = setup(DOC);
    let read = || std::fs::read_to_string(dir.path().join("paper.md")).unwrap();
    let s = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]));
    assert!(error(dir.path(), &["revert", &s]).contains("only accepted"));
    annox(dir.path(), &["accept", &s]);

    // Without --accept, the revert waits for review.
    let r = annox(dir.path(), &["revert", &s, "--body", "keep the typo"]);
    assert_eq!(r["reverts"], serde_json::json!(s));
    let shown = annox(dir.path(), &["show", &id(&r)]);
    assert_eq!(
        (&shown["replacement"], &shown["quote"], &shown["reverts"]),
        (&"teh".into(), &"the".into(), &s.as_str().into())
    );
    assert_eq!(read(), DOC.replace("teh", "the"));

    assert_eq!(id(&annox(dir.path(), &["revert", &s])), id(&r), "the open revert is used again");
    let accepted = annox(dir.path(), &["revert", &s, "--accept"]);
    assert_eq!(
        (&accepted["id"], &accepted["status"], &accepted["reverts"]),
        (&r["id"], &"accepted".into(), &s.as_str().into())
    );
    assert_eq!(read(), DOC);
    assert!(error(dir.path(), &["revert", &s]).contains(&format!("already reverted by {}", id(&r))));

    // A deletion is reverted by an insertion.
    let d =
        id(&annox(dir.path(), &["suggest", "paper.md", "--quote", " is tight", "--occurrence", "1", "--replace", ""]));
    annox(dir.path(), &["accept", &d]);
    annox(dir.path(), &["revert", &d, "--accept"]);
    assert_eq!(read(), DOC);

    // An accepted suggestion whose text changed since can't be found to revert.
    let e = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]));
    annox(dir.path(), &["accept", &e]);
    std::fs::write(dir.path().join("paper.md"), DOC.replace("teh", "a")).unwrap();
    assert!(error(dir.path(), &["revert", &e]).contains("changed since"));
}

#[test]
fn accept_keeps_bom_and_line_endings() {
    let dir = setup("\u{feff}we prove that teh bound\r\nis tight.\r\n");
    let s = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "teh", "--replace", "the\nnew"]));
    assert!(error(dir.path(), &["status", &s, "accepted"]).contains("annox accept"));
    let accepted = annox(dir.path(), &["accept", &s]);
    let raw = std::fs::read_to_string(dir.path().join("paper.md")).unwrap();
    assert_eq!(raw, "\u{feff}we prove that the\r\nnew bound\r\nis tight.\r\n");
    let version = annox_core::text::Text::from_raw(&raw).version;
    assert_eq!(accepted["appliedVersion"], version);

    let all = annox(dir.path(), &["list", "--all"]);
    assert_eq!(all[0]["status"], "accepted");
    assert!(all[0].get("resolution").is_none(), "closed suggestions aren't resolved");
    assert!(error(dir.path(), &["accept", &s]).contains("only open suggestions"));
}

#[test]
fn accepting_carries_comments_on_the_replaced_text() {
    let dir = setup(DOC);
    let c = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "prove that teh bound", "--body", "x"]));
    let s = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]));
    let accepted = annox(dir.path(), &["accept", &s]);
    assert_eq!(accepted["reanchored"], serde_json::json!([c]));
    let list = annox(dir.path(), &["list"]);
    assert_eq!((&list[0]["quote"], &list[0]["resolution"]), (&"prove that the bound".into(), &"exact".into()));
}

#[test]
fn edited_quotes_are_found_between_their_prefix_and_suffix() {
    let dir = setup(DOC);
    annox(dir.path(), &["comment", "paper.md", "--quote", "we prove that", "--body", "x"]);
    std::fs::write(dir.path().join("paper.md"), DOC.replace("we prove", "we formally prove")).unwrap();
    assert_eq!(annox(dir.path(), &["list"])[0]["resolution"], "relocated");
}

#[test]
fn relocated_suggestions_need_confirmation() {
    let dir = setup(DOC);
    let s = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]));
    // Rewrite everything around the quote, so only a quote search finds it.
    std::fs::write(dir.path().join("paper.md"), "Something else.\nNow teh end.\n").unwrap();
    let err = error(dir.path(), &["accept", &s]);
    assert!(err.contains("line 2") && err.contains("--confirmed"), "{err}");
    annox(dir.path(), &["accept", &s, "--confirmed"]);
    assert_eq!(std::fs::read_to_string(dir.path().join("paper.md")).unwrap(), "Something else.\nNow the end.\n");

    let t = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "end", "--replace", "start"]));
    std::fs::write(dir.path().join("paper.md"), "Nothing here.\n").unwrap();
    assert!(error(dir.path(), &["accept", &t]).contains("stale"));
    assert_eq!(annox(dir.path(), &["list"])[0]["resolution"], "orphaned");
}

#[test]
fn rejects_unknown_options() {
    let dir = setup(DOC);
    let err = error(dir.path(), &["comment", "paper.md", "--quote", "teh", "--bdy", "x"]);
    assert!(err.contains("unexpected argument '--bdy'") && err.contains("'--body'"), "{err}");
}

#[test]
fn values_may_start_with_a_hyphen() {
    let dir = setup("- item one\n- item two\n");
    let s = id(&annox(
        dir.path(),
        &["suggest", "paper.md", "--quote", "- item two", "--replace", "- item 2", "--body", "-"],
    ));
    let list = annox(dir.path(), &["list"]);
    assert_eq!((&list[0]["id"], &list[0]["replacement"]), (&s.into(), &"- item 2".into()));
}

#[test]
fn retarget_and_reattach_rescue_stale_annotations() {
    let dir = setup(DOC);
    let s = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]));
    let c = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "Section 3", "--body", "Which?"]));
    std::fs::write(dir.path().join("paper.md"), "In Chapter 3, we prove that tha bound holds.\n").unwrap();
    let list = annox(dir.path(), &["list"]);
    assert!(list.as_array().unwrap().iter().all(|a| a["resolution"] == "orphaned"));

    assert!(error(dir.path(), &["retarget", &c, "--quote", "Chapter 3", "--replace", "x"]).contains("annox reattach"));
    assert!(error(dir.path(), &["reattach", &s, "--quote", "tha"]).contains("annox retarget"));
    annox(dir.path(), &["retarget", &s, "--quote", "tha bound", "--replace", "the bound"]);
    annox(dir.path(), &["reattach", &c, "--quote", "Chapter 3"]);
    let list = annox(dir.path(), &["list"]);
    let by_kind = |k: &str| list.as_array().unwrap().iter().find(|a| a["kind"] == k).unwrap().clone();
    assert_eq!(
        (&by_kind("suggestion")["quote"], &by_kind("suggestion")["applicable"]),
        (&"tha bound".into(), &true.into())
    );
    assert_eq!(
        (&by_kind("comment")["quote"], &by_kind("comment")["resolution"]),
        (&"Chapter 3".into(), &"exact".into())
    );

    annox(dir.path(), &["accept", &s]);
    assert!(error(dir.path(), &["retarget", &s, "--quote", "the", "--replace", "a"]).contains("only open"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("paper.md")).unwrap(),
        "In Chapter 3, we prove that the bound holds.\n"
    );
}

#[test]
fn reattach_to_the_suggested_location() {
    let dir = setup("# Plan\n\nExisting projects are backfilled to NULL, which means the in-process legacy path.\n\nNext: workers.\n");
    let c = id(&annox(
        dir.path(),
        &[
            "comment",
            "paper.md",
            "--quote",
            "Existing projects are backfilled to NULL, which means the in-process legacy path.",
            "--body",
            "Why?",
        ],
    ));
    assert!(error(dir.path(), &["reattach", &c, "--suggested"]).contains("isn't orphaned"));
    std::fs::write(dir.path().join("paper.md"), "# Plan\n\nAll existing projects are backfilled to the first line, which means there is no legacy path.\n\nWorkers come next.\n").unwrap();
    let list = annox(dir.path(), &["list", "--broken"]);
    let suggested = &list[0]["suggested"];
    assert_eq!(suggested["line"], 3);
    assert_eq!(
        suggested["quote"],
        "existing projects are backfilled to the first line, which means there is no legacy path."
    );
    assert!(error(dir.path(), &["reattach", &c, "--suggested", "--quote", "x"]).contains("cannot be used with"));

    annox(dir.path(), &["reattach", &c, "--suggested"]);
    let list = annox(dir.path(), &["list"]);
    assert_eq!(list[0]["resolution"], "exact");
    assert_eq!(list[0]["quote"], suggested["quote"]);
    assert!(list[0].get("suggested").is_none());

    // Nothing left to suggest once the text is gone.
    std::fs::write(dir.path().join("paper.md"), "# Plan\n\nTBD.\n").unwrap();
    assert!(annox(dir.path(), &["list"])[0].get("suggested").is_none());
    assert!(error(dir.path(), &["reattach", &c, "--suggested"]).contains("no suggested location"));
}

#[test]
fn restore_undoes_delete() {
    let dir = setup(DOC);
    let c = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "Section 3", "--body", "Which?"]));
    annox(dir.path(), &["delete", &c]);
    assert_eq!(annox(dir.path(), &["list"]), serde_json::json!([]));
    annox(dir.path(), &["restore", &c]);
    assert_eq!(annox(dir.path(), &["list"])[0]["id"], c.as_str());
}

#[test]
fn list_filters() {
    let dir = setup(DOC);
    let ids = |args: &[&str]| -> Vec<String> {
        let mut ids: Vec<String> = annox(dir.path(), args).as_array().unwrap().iter().map(id).collect();
        ids.sort();
        ids
    };
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    let c = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "Section 3", "--body", "Which?"]));
    let s = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]));
    let args = ["comment", "paper.md", "--quote", "prove", "--body", "x", "--author", "urn:test:user"];
    let theirs = id(&cli::run(&args.map(String::from), dir.path()).unwrap());
    assert_eq!(ids(&["list", "--kind", "suggestion"]), vec![s.clone()]);
    assert_eq!(ids(&["list", "--kind", "comment"]), sorted(vec![c.clone(), theirs.clone()]));
    assert_eq!(ids(&["list", "--author", "urn:test:agent", "--kind", "comment"]), vec![c.clone()]);
    assert_eq!(ids(&["list", "--not-author", "urn:test:agent"]), vec![theirs.clone()]);
    assert_eq!(ids(&["list", "--author", "urn:test:user", "--author", "urn:test:agent", "--kind", "comment"]).len(), 2);
    annox(dir.path(), &["delete", &theirs]);
    assert!(error(dir.path(), &["list", "--kind", "reply"]).contains("invalid value"));

    annox(dir.path(), &["status", &c, "resolved"]);
    assert_eq!(ids(&["list", "--status", "resolved"]), vec![c.clone()]);
    assert_eq!(ids(&["list", "--status", "open,resolved"]), sorted(vec![c.clone(), s.clone()]));

    assert_eq!(ids(&["list", "--broken"]), Vec::<String>::new());
    std::fs::write(dir.path().join("paper.md"), "Nothing here.\n").unwrap();
    assert_eq!(ids(&["list", "--broken"]), vec![s.clone()]);
    assert!(!ids(&["list", "--broken", "--all"]).contains(&c), "closed annotations aren't broken");
}

#[test]
fn list_by_quote_includes_closed_annotations() {
    let dir = setup(DOC);
    let list = |args: &[&str]| -> Vec<Value> { annox(dir.path(), args).as_array().unwrap().clone() };
    let ids = |args: &[&str]| -> Vec<String> {
        let mut ids: Vec<String> = list(args).iter().map(id).collect();
        ids.sort();
        ids
    };
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    let elsewhere = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "Section 3", "--body", "Which?"]));
    let accepted = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]));
    annox(dir.path(), &["accept", &accepted]);
    let open = id(&annox(
        dir.path(),
        &["comment", "paper.md", "--quote", "bound is tight", "--occurrence", "1", "--body", "x"],
    ));
    let rejected = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "prove", "--replace", "show"]));
    annox(dir.path(), &["status", &rejected, "rejected"]);
    let deleted = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "we prove", "--body", "x"]));
    annox(dir.path(), &["delete", &deleted]);
    let deletion =
        id(&annox(dir.path(), &["suggest", "paper.md", "--quote", " is tight", "--occurrence", "2", "--replace", ""]));
    annox(dir.path(), &["accept", &deletion]);

    let quote = ["list", "paper.md", "--quote", "prove that the bound"];
    assert_eq!(ids(&quote), sorted(vec![accepted.clone(), open.clone(), rejected.clone()]));
    let shown = list(&quote).into_iter().find(|v| v["id"] == accepted.as_str()).unwrap();
    assert_eq!((&shown["line"], &shown["quote"], &shown["replacement"]), (&1.into(), &"teh".into(), &"the".into()));
    assert!(!ids(&quote).contains(&elsewhere));
    assert!(ids(&[&quote[..], &["--all"]].concat()).contains(&deleted));
    assert_eq!(ids(&[&quote[..], &["--status", "open"]].concat()), vec![open.clone()]);
    assert_eq!(
        ids(&["list", "paper.md", "--quote", "The bound."]),
        vec![deletion],
        "an accepted deletion is where it cut"
    );
    assert!(error(dir.path(), &["list", "--quote", "prove"]).contains("FILE"));
}

#[test]
fn show_prints_one_thread() {
    let dir = setup(DOC);
    let c = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "Section 3", "--body", "Which?"]));
    let r = id(&annox(dir.path(), &["reply", &c, "--body", "The third."]));
    annox(dir.path(), &["status", &c, "resolved"]);
    let shown = annox(dir.path(), &["show", &c]);
    assert_eq!((&shown["status"], &shown["replies"][0]["body"]), (&"resolved".into(), &"The third.".into()));
    assert_eq!(annox(dir.path(), &["show", &r]), shown, "a reply shows its thread");
    assert!(error(dir.path(), &["show", "nope"]).contains("unknown annotation"));
}

#[test]
fn history_lists_who_did_what() {
    let dir = setup(DOC);
    let c = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "Section 3", "--body", "Which?"]));
    let s = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]));
    let r = id(&annox(dir.path(), &["reply", &c, "--body", "The third."]));
    annox(dir.path(), &["status", &c, "resolved"]);
    annox(dir.path(), &["edit", &r, "--body", "The third one."]);
    annox(dir.path(), &["status", &c, "open"]);
    annox(dir.path(), &["accept", &s]);

    let events = annox(dir.path(), &["history", &c]);
    let types: Vec<&str> = events.as_array().unwrap().iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["create", "create", "status", "edit", "status"], "the thread, interleaved");
    assert_eq!(annox(dir.path(), &["history", &r]), events, "a reply shows its thread");
    let shown = annox(dir.path(), &["show", &c]);
    assert_eq!(shown["editedBy"], Value::Null);
    assert_eq!(shown["replies"][0]["editedBy"]["author"]["name"], "Agent");
    let text = plain(render::history(&events));
    let actions: Vec<&str> = text.lines().map(|l| l.split("  ").nth(2).unwrap()).collect();
    assert_eq!(
        actions,
        ["commented: Which?", "replied: The third.", "resolved", "edited reply: The third one.", "reopened"]
    );
    assert!(text.lines().all(|l| l.contains("  Agent  ")), "{text}");

    let text = plain(render::history(&annox(dir.path(), &["history", &s])));
    assert_eq!(text.lines().map(|l| l.split("  ").nth(2).unwrap()).collect::<Vec<_>>(), ["suggested: the", "accepted"]);
    assert!(error(dir.path(), &["history", "nope"]).contains("unknown annotation"));
}

#[test]
fn init_local_keeps_the_workspace_out_of_git() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(dir.path()).output().unwrap();
    git(&["init", "-q"]);
    std::fs::write(dir.path().join("paper.md"), DOC).unwrap();
    annox(dir.path(), &["init", "--local"]);
    annox(dir.path(), &["comment", "paper.md", "--quote", "teh", "--body", "typo"]);
    let status = String::from_utf8(git(&["status", "--porcelain", "--untracked-files=all"]).stdout).unwrap();
    assert_eq!(status, "?? paper.md\n");
    // Running it again on an existing workspace doesn't repeat the line.
    annox(dir.path(), &["init", "--local"]);
    let ignore = std::fs::read_to_string(dir.path().join(".annox/.gitignore")).unwrap();
    assert_eq!(ignore.lines().filter(|l| *l == "*").count(), 1);
}

#[test]
fn report_counts_each_document() {
    let dir = setup(DOC);
    std::fs::write(dir.path().join("notes.md"), "Some notes.\n").unwrap();
    let c = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "Section 3", "--body", "Which?"]));
    annox(dir.path(), &["comment", "paper.md", "--quote", "prove", "--body", "x"]);
    annox(dir.path(), &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]);
    annox(dir.path(), &["status", &c, "resolved"]);
    let gone = id(&annox(dir.path(), &["comment", "notes.md", "--quote", "notes", "--body", "y"]));
    annox(dir.path(), &["comment", "notes.md", "--quote", "Some", "--body", "z"]);
    annox(dir.path(), &["delete", &gone]);
    std::fs::remove_file(dir.path().join("notes.md")).unwrap();

    let report = annox(dir.path(), &["report"]);
    let docs = report["documents"].as_array().unwrap();
    assert_eq!(docs.len(), 2);
    let (notes, paper) = (&docs[0], &docs[1]);
    assert_eq!((&notes["path"], &notes["missing"], &notes["orphaned"]), (&"notes.md".into(), &true.into(), &1.into()));
    assert_eq!(notes["comments"]["open"], 1, "deleted annotations aren't counted");
    assert_eq!((&paper["comments"]["open"], &paper["comments"]["resolved"]), (&1.into(), &1.into()));
    assert_eq!((&paper["suggestions"]["open"], &paper["stale"]), (&1.into(), &0.into()));
    assert_eq!(annox(dir.path(), &["report", "paper.md"])["documents"].as_array().unwrap().len(), 1);

    let text = plain(render::report(&report));
    assert_eq!(
        text,
        "notes.md  file missing · 1 open comment · 1 orphaned\n\
         paper.md  1 open suggestion · 1 open comment · 1 closed\n"
    );
    assert_eq!(plain(render::report(&serde_json::json!({ "documents": [] }))), "no annotations\n");

    assert_eq!(
        plain(render::report_verbose(&report)),
        "notes.md  (file missing)\n  comments     1 open\n  needs work   1 orphaned\n\
         paper.md\n  suggestions  1 open\n  comments     1 open · 1 resolved\n"
    );
    assert_eq!(plain(render::report_verbose(&serde_json::json!({ "documents": [] }))), "no annotations\n");
}

#[test]
fn report_groups_by_author() {
    let dir = setup(DOC);
    let ada = |name: &str, args: &[&str]| {
        let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        args.extend(["--author", "mailto:ada@example.org", "--name", name].map(String::from));
        cli::run(&args, dir.path()).unwrap()
    };
    let c = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "Section 3", "--body", "Which?"]));
    ada("Ada", &["reply", &c, "--body", "The third."]);
    ada("Ada Lovelace", &["reply", &c, "--body", "Or the fourth."]);
    let s = id(&ada("Ada Lovelace", &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]));
    annox(dir.path(), &["reply", &s, "--body", "Yes."]);
    let done = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "prove", "--body", "x"]));
    annox(dir.path(), &["status", &done, "resolved"]);

    let report = annox(dir.path(), &["report"]);
    assert_eq!(report["documents"][0]["replies"], 3, "replies count on documents too");

    let report = annox(dir.path(), &["report", "--by", "author"]);
    let authors = report["authors"].as_array().unwrap();
    assert_eq!(authors.len(), 2);
    let (ada_entry, agent) = (&authors[0], &authors[1]);
    assert_eq!(ada_entry["author"], serde_json::json!({ "id": "mailto:ada@example.org", "name": "Ada Lovelace" }));
    assert_eq!((&ada_entry["suggestions"]["open"], &ada_entry["comments"]["open"]), (&1.into(), &0.into()));
    assert_eq!(ada_entry["replies"], 2);
    assert_eq!((&agent["comments"]["open"], &agent["comments"]["resolved"]), (&1.into(), &1.into()));
    assert_eq!(agent["replies"], 1);
    assert_eq!(
        plain(render::report(&report)),
        "Ada Lovelace <mailto:ada@example.org>  1 open suggestion · 2 replies\n\
         Agent <urn:test:agent>                 1 open comment · 1 reply · 1 closed\n"
    );

    let nested = annox(dir.path(), &["report", "--by", "author,document"]);
    assert_eq!(nested["authors"][0]["documents"][0]["path"], "paper.md");
    assert_eq!(nested["authors"][0]["documents"][0]["replies"], 2);
    assert_eq!(
        plain(render::report(&nested)),
        "Ada Lovelace <mailto:ada@example.org>  1 open suggestion · 2 replies\n\
         \x20 paper.md                             1 open suggestion · 2 replies\n\
         Agent <urn:test:agent>                 1 open comment · 1 reply · 1 closed\n\
         \x20 paper.md                             1 open comment · 1 reply · 1 closed\n"
    );
    let nested = annox(dir.path(), &["report", "--by", "document,author"]);
    assert_eq!(nested["documents"][0]["authors"].as_array().unwrap().len(), 2);

    // Filters pick the threads; grouping then counts everyone in them.
    let mine = annox(dir.path(), &["report", "--by", "author", "--mine", "--as", "urn:test:agent"]);
    assert_eq!(mine["authors"][0]["author"]["id"], "urn:test:agent");
    assert_eq!(mine["authors"][0]["suggestions"]["open"], 0, "Ada's suggestion is filtered out");
    assert_eq!(
        (&mine["authors"][1]["author"]["id"], &mine["authors"][1]["replies"]),
        (&"mailto:ada@example.org".into(), &2.into())
    );

    assert!(error(dir.path(), &["report", "--by", "author,author"]).contains("--by author is given twice"));
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn commit_takes_only_annotation_events() {
    let dir = setup(DOC);
    assert!(error(dir.path(), &["commit"]).contains("not a git repository"));
    assert!(annox(dir.path(), &["report"]).get("uncommitted").is_none());

    git(dir.path(), &["init", "--quiet"]);
    git(dir.path(), &["config", "user.email", "ada@example.org"]);
    git(dir.path(), &["config", "user.name", "Ada"]);
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    let c = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "Section 3", "--body", "Which one?"]));
    annox(dir.path(), &["reply", &c, "--body", "The third."]);
    annox(dir.path(), &["comment", "paper.md", "--quote", "teh", "--body", "draft", "--local"]);
    git(dir.path(), &["add", "paper.md"]);

    let report = annox(dir.path(), &["report"]);
    // annox.json, .gitignore, the document event, and two annotation events.
    assert_eq!((&report["uncommitted"], &report["documents"][0]["uncommitted"]), (&5.into(), &2.into()));
    assert!(plain(render::report(&report)).contains("2 uncommitted changes"));
    assert!(plain(render::report(&report)).ends_with("5 files to commit; see `annox commit --dry-run`\n"));
    let verbose = plain(render::report_verbose(&report));
    assert!(verbose.contains("\n  uncommitted  2 changes\n"));
    assert!(verbose.ends_with("5 files to commit; see `annox commit --dry-run`\n"));

    let planned = annox(dir.path(), &["commit", "--dry-run"]);
    assert_eq!(planned["commit"], Value::Null);
    assert_eq!(planned["message"], "chore(annox): 1 comment, 1 reply on paper.md");
    assert_eq!(planned["files"], 5);
    assert_eq!(
        planned["documents"],
        serde_json::json!({ "paper.md": { "comments": 1, "suggestions": 0, "replies": 1, "updates": 0 } })
    );
    assert_eq!(git(dir.path(), &["diff", "--cached", "--name-only"]), "paper.md\n", "a dry run stages nothing");

    let committed = annox(dir.path(), &["commit"]);
    assert_eq!((&committed["message"], &committed["files"]), (&planned["message"], &planned["files"]));
    let files = git(dir.path(), &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(files.lines().count(), 5);
    assert!(files.lines().all(|f| f.starts_with(".annox/") && !f.starts_with(".annox/local/")), "{files}");
    // What the user had staged stays staged, and out of the commit.
    assert_eq!(git(dir.path(), &["diff", "--cached", "--name-only"]), "paper.md\n");
    assert_eq!(annox(dir.path(), &["report"])["uncommitted"], 0);

    assert_eq!(annox(dir.path(), &["commit"])["commit"], Value::Null);
    annox(dir.path(), &["status", &c, "resolved"]);
    let committed = annox(dir.path(), &["commit", "-m", "Resolve review"]);
    assert_eq!(committed["message"], "Resolve review");
    assert_eq!(committed["files"], 1);
}

#[test]
fn unread_is_kept_per_reader() {
    let dir = setup(DOC);
    let me = "mailto:ada@example.org";
    let unread = |reader: &str| annox(dir.path(), &["list", "--unread", "--as", reader]);
    let as_ada = |args: &[&str]| {
        let args: Vec<String> = args.iter().chain(&["--author", me]).map(|s| s.to_string()).collect();
        cli::run(&args, dir.path()).unwrap()
    };
    // Both readers start from a baseline: nothing is unread.
    assert_eq!(unread(me), serde_json::json!([]));
    assert_eq!(unread("urn:test:agent"), serde_json::json!([]));

    // The agent comments: new to Ada, not to the agent.
    let comment = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "teh", "--body", "Typo?"]));
    assert_eq!(unread(me)[0]["id"], comment.as_str());
    assert_eq!(unread(me)[0]["unread"], true);
    assert_eq!(unread("urn:test:agent"), serde_json::json!([]));

    // The agent marking everything read leaves Ada's state alone.
    annox(dir.path(), &["read", "--all"]);
    assert_eq!(unread(me).as_array().unwrap().len(), 1);

    // Ada replies, which marks the thread read for her and is new to the agent.
    let reply = as_ada(&["reply", &comment, "--body", "Yes"]);
    assert_eq!(unread(me), serde_json::json!([]));
    let agent = unread("urn:test:agent");
    assert_eq!(agent[0]["unread"], false);
    assert_eq!(agent[0]["replies"][0]["id"], reply["id"]);
    assert_eq!(agent[0]["replies"][0]["unread"], true);

    // Resolved threads with news are still listed.
    as_ada(&["status", &comment, "resolved"]);
    assert_eq!(unread("urn:test:agent")[0]["status"], "resolved");
    annox(dir.path(), &["read", &comment]);
    assert_eq!(unread("urn:test:agent"), serde_json::json!([]));
    assert!(error(dir.path(), &["read", "nope"]).contains("unknown annotation"));
}

#[test]
fn watch_reports_threads_addressed_to_the_reader() {
    use std::time::{Duration, Instant};
    let dir = setup(DOC);
    let as_user = |args: &[&str]| {
        let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        args.extend(["--author".into(), "urn:test:user".into()]);
        cli::run(&args, dir.path()).unwrap()
    };
    // Already there when the agent starts watching.
    as_user(&["comment", "paper.md", "--quote", "Section 3", "--body", "@agent before"]);
    let options = cli::Watch {
        reader: Some("urn:test:agent".into()),
        mention: vec!["@Agent".into()],
        settle: 1000,
        ..cli::Watch::default()
    };
    let mut watcher = cli::Watcher::new(options, dir.path()).unwrap();
    let start = Instant::now();
    let ids = |views: Vec<Value>| views.iter().map(id).collect::<Vec<_>>();
    assert_eq!(ids(watcher.poll(start).unwrap()), Vec::<String>::new(), "a new reader starts with nothing unread");

    let asked = id(&as_user(&["comment", "paper.md", "--quote", "teh bound", "--body", "@agent typo?"]));
    as_user(&["comment", "paper.md", "--quote", "Section 3", "--occurrence", "1", "--body", "not for the agent"]);
    assert_eq!(ids(watcher.poll(start).unwrap()), Vec::<String>::new(), "it waits for the thread to settle");
    let later = start + Duration::from_millis(1100);
    assert_eq!(ids(watcher.poll(later).unwrap()), vec![asked.clone()]);
    assert_eq!(ids(watcher.poll(later).unwrap()), Vec::<String>::new(), "reported once");

    // The agent answers: its own writes don't make the thread unread.
    annox(dir.path(), &["reply", &asked, "--body", "Fixed in a suggestion."]);
    assert_eq!(ids(watcher.poll(later).unwrap()), Vec::<String>::new());
    // A reply on a thread the agent wrote in is addressed to it, mention or not.
    as_user(&["reply", &asked, "--body", "Thanks!"]);
    watcher.poll(later).unwrap();
    let much_later = later + Duration::from_millis(1100);
    let reported = watcher.poll(much_later).unwrap();
    assert_eq!(ids(reported.clone()), vec![asked.clone()]);
    assert_eq!(reported[0]["replies"][1]["unread"], true);

    annox(dir.path(), &["read", &asked]);
    assert_eq!(ids(watcher.poll(much_later).unwrap()), Vec::<String>::new());
}

#[test]
fn log_lists_and_follows_every_event() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    let dir = setup(DOC);
    std::fs::write(dir.path().join("other.md"), "Another document.\n").unwrap();
    let c = id(&annox(dir.path(), &["comment", "paper.md", "--quote", "Section 3", "--body", "Which one?"]));
    let r = id(&annox(dir.path(), &["reply", &c, "--body", "The third."]));
    annox(dir.path(), &["edit", &r, "--body", "The third one."]);
    annox(dir.path(), &["comment", "other.md", "--quote", "Another", "--body", "Elsewhere."]);

    let log = annox(dir.path(), &["log", "paper.md"]);
    let types: Vec<_> = log.as_array().unwrap().iter().map(|e| (e["type"].clone(), e["thread"].clone())).collect();
    assert_eq!(
        types,
        vec![
            ("create".into(), c.clone().into()),
            ("create".into(), c.clone().into()),
            ("edit".into(), c.clone().into())
        ]
    );
    assert_eq!(log[0]["path"], "paper.md");
    assert_eq!(annox(dir.path(), &["log"]).as_array().unwrap().len(), 4);
    let text = plain(render::log(&log));
    assert!(text.lines().nth(2).unwrap().ends_with("edited reply: The third one."), "{text}");

    // Following prints only what's written after it starts, by anyone.
    let stop = AtomicBool::new(false);
    let seen = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        scope.spawn(|| {
            cli::follow_log(None, dir.path(), &stop, |e| {
                seen.lock().unwrap().push(e);
                Ok(())
            })
            .unwrap()
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        annox(dir.path(), &["status", &c, "resolved"]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while seen.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        stop.store(true, Ordering::Relaxed);
    });
    let seen = seen.into_inner().unwrap();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        (&seen[0]["type"], &seen[0]["status"], &seen[0]["author"]["id"]),
        (&"status".into(), &"resolved".into(), &"urn:test:agent".into())
    );
}

/// A renderer's output without its colors.
fn plain(text: String) -> String {
    anstream::adapter::strip_str(&text).to_string()
}

#[test]
fn reactions_and_approvals() {
    let dir = setup(DOC);
    let s = id(&annox(dir.path(), &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]));
    let reacted = annox(dir.path(), &["react", &s, "👍"]);
    assert_eq!(reacted["changed"], true);
    // Reacting again changes nothing.
    assert_eq!(annox(dir.path(), &["react", &s, "👍"])["changed"], false);
    let list = annox(dir.path(), &["list"]);
    assert_eq!(list[0]["reactions"]["👍"][0]["author"]["id"], "urn:test:agent");
    assert_eq!(list[0]["approvals"][0]["stale"], false);

    // Re-targeting makes the approval stale.
    annox(dir.path(), &["retarget", &s, "--quote", "teh bound", "--replace", "the bound"]);
    assert_eq!(annox(dir.path(), &["list"])[0]["approvals"][0]["stale"], true);

    annox(dir.path(), &["react", &s, "👍", "--remove"]);
    let list = annox(dir.path(), &["list"]);
    assert!(list[0].get("reactions").is_none() && list[0].get("approvals").is_none(), "{list}");

    let history = render::history(&annox(dir.path(), &["history", &s]));
    assert!(history.contains("reacted 👍") && history.contains("removed 👍"), "{history}");
    assert!(error(dir.path(), &["react", &s, ""]).contains("empty"));

    // Approved open suggestions are counted in the report.
    annox(dir.path(), &["react", &s, "👍"]);
    let report = annox(dir.path(), &["report"]);
    assert_eq!(report["documents"][0]["approved"], 1);
    assert_eq!(plain(render::report(&report)), "paper.md  1 open suggestion · 1 approved\n");
    assert_eq!(plain(render::report_verbose(&report)), "paper.md\n  suggestions  1 open · 1 approved\n");
}

/// What `annox ARGS` prints as text in `cwd`.
fn text(cwd: &Path, args: &[&str]) -> String {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let render = render::of(&cli::parse(&args).unwrap());
    plain(render(&annox(cwd, &args.iter().map(String::as_str).collect::<Vec<_>>())))
}

#[test]
fn prints_threads_as_text() {
    let dir = setup("# Title\n\nThe bound is tight.\nThe proof is short.\n");
    let d = dir.path();
    let tight = id(&annox(d, &["comment", "paper.md", "--quote", "tight", "--body", "Prove it.\n\nOr cite it."]));
    let reply = id(&annox(d, &["reply", &tight, "--body", "Done"]));
    let short = id(&annox(d, &["suggest", "paper.md", "--quote", "short", "--replace", "long", "--label", "typo"]));
    let mark = id(&annox(d, &["highlight", "paper.md", "--quote", "proof"]));
    assert_eq!(
        text(d, &["list", "paper.md"]),
        format!(
            "paper.md\n  3  open  “tight”  {tight}\n     Agent: Prove it.\n\n       Or cite it.\n     ↳ Agent: Done\n  \
             4  open  “short” → “long”  [typo]  {short}\n     Agent suggested it\n  \
             4  open  “proof”  {mark}\n     Agent highlighted it\n"
        )
    );
    // A reply's thread, as `list` prints it.
    assert_eq!(text(d, &["show", &reply]), text(d, &["list", "paper.md", "--quote", "tight"]));
    assert_eq!(text(d, &["list", "paper.md", "--kind", "comment", "--status", "resolved"]), "no annotations\n");
}

#[test]
fn prints_what_writes_did_as_text() {
    let dir = setup("# Title\n\nThe bound is tight.\n");
    let d = dir.path();
    let out = text(d, &["comment", "paper.md", "--quote", "tight", "--body", "Why?"]);
    let tight = out.rsplit("  ").next().unwrap().trim().to_owned();
    assert_eq!(out, format!("Commented on paper.md:3  {tight}\n"));
    assert_eq!(text(d, &["status", &tight, "resolved"]), format!("Resolved  {tight}\n"));
    assert_eq!(text(d, &["react", &tight, "👍"]), format!("Reacted 👍 to {tight}\n"));
    assert_eq!(text(d, &["react", &tight, "👍"]), format!("{tight} already has your 👍\n"));
    assert_eq!(text(d, &["read", "--all"]), "Marked 1 thread read\n");
}

/// A PDF with a page per entry of `pages`, each showing its lines in
/// Helvetica, one per line from the top.
fn write_pdf(path: &Path, pages: &[&[&str]]) {
    write_pdf_in(path, pages, "Helvetica");
}

/// [`write_pdf`] in `font`. In Courier, each character of line `i` is
/// [`COURIER`] wide, from x = 72, with its baseline at y = 700 − 20i.
fn write_pdf_in(path: &Path, pages: &[&[&str]], font: &str) {
    use lopdf::{dictionary, Document, Object, Stream};
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => font, "Encoding" => "WinAnsiEncoding",
    });
    let mut kids = Vec::new();
    for lines in pages {
        let mut ops = String::new();
        for (i, line) in lines.iter().enumerate() {
            ops.push_str(&format!("BT /F1 12 Tf 72 {} Td ({line}) Tj ET\n", 700 - 20 * i));
        }
        let content = doc.add_object(Stream::new(dictionary! {}, ops.into_bytes()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        });
        kids.push(page.into());
    }
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Count" => kids.len() as i64, "Kids" => kids,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    doc.save(path).unwrap();
}

/// The annotations of page `n` of the PDF at `path`, by `/NM`.
fn pdf_annots(path: &Path, n: u32) -> Vec<(String, lopdf::Dictionary)> {
    let doc = lopdf::Document::load(path).unwrap();
    let page = doc.get_pages()[&n];
    let annots = doc.get_dictionary(page).unwrap().get(b"Annots").map(|a| match a {
        lopdf::Object::Reference(r) => doc.get_object(*r).unwrap().as_array().unwrap().clone(),
        a => a.as_array().unwrap().clone(),
    });
    annots
        .unwrap_or_default()
        .iter()
        .map(|a| {
            let d = doc.get_dictionary(a.as_reference().unwrap()).unwrap().clone();
            // Links have no name.
            let nm = d.get(b"NM").map_or(String::new(), |n| String::from_utf8(n.as_str().unwrap().to_vec()).unwrap());
            (nm, d)
        })
        .collect()
}

#[test]
fn export_pdf_places_threads_on_their_text() {
    let dir = setup("# Title\n\nThe bound is *tight*. The bound is loose.\n\nGone from the PDF.\n");
    let d = dir.path();
    write_pdf(&d.join("paper.pdf"), &[&["Title", "The bound is tight. The bound is", "loose."]]);
    let tight = id(&annox(d, &["comment", "paper.md", "--quote", "bound is *tight*", "--body", "Prove it"]));
    annox(d, &["reply", &tight, "--body", "Done"]);
    let loose = id(&annox(d, &["suggest", "paper.md", "--quote", "bound is loose", "--replace", "bound is slack"]));
    let gone = id(&annox(d, &["comment", "paper.md", "--quote", "Gone", "--body", "Where?"]));

    let out = annox(d, &["export-pdf", "paper.md", "paper.pdf"]);
    assert_eq!(out["placed"], serde_json::json!([tight, loose]));
    assert_eq!(out["unplaced"], serde_json::json!([gone]));
    let output = d.join("paper.annotated.pdf");
    assert_eq!(Path::new(out["output"].as_str().unwrap()), output.canonicalize().unwrap());

    let page = pdf_annots(&output, 1);
    let get = |nm: &str| &page.iter().find(|(n, _)| n == nm).unwrap_or_else(|| panic!("no {nm}")).1;
    let quads = |d: &lopdf::Dictionary| -> Vec<f32> {
        d.get(b"QuadPoints").unwrap().as_array().unwrap().iter().map(|v| v.as_float().unwrap()).collect()
    };
    let highlight = get(&tight);
    assert_eq!(highlight.get(b"Subtype").unwrap().as_name().unwrap(), b"Highlight");
    // On the second line, starting after "The ".
    let q = quads(highlight);
    assert!(q.len() == 8 && q[0] > 95.0 && q[0] < 110.0 && q[5] > 675.0 && q[1] < 700.0, "{q:?}");
    let reply = &page
        .iter()
        .find(|(_, d)| d.get(b"IRT").is_ok() && d.get(b"Subtype").unwrap().as_name().unwrap() == b"Text")
        .unwrap()
        .1;
    assert_eq!(reply.get(b"Contents").unwrap().as_str().unwrap(), b"Done");
    // A suggestion across a line break strikes its whole quote, a quad per
    // line, and a caret grouped with it draws the replacement.
    let strike = get(&loose);
    assert_eq!(strike.get(b"Subtype").unwrap().as_name().unwrap(), b"StrikeOut");
    // The page shows the whole change and there's no reason, so there's no note.
    assert!(strike.get(b"Contents").is_err());
    assert_eq!(quads(strike).len(), 16);
    let caret = get(&format!("{loose}-caret"));
    assert!(caret.get(b"IRT").is_ok());
    // It draws the replacement itself, above the struck text.
    assert!(caret.get(b"AP").unwrap().as_dict().unwrap().get(b"N").is_ok());
    let rect: Vec<f32> =
        caret.get(b"Rect").unwrap().as_array().unwrap().iter().map(|v| v.as_float().unwrap()).collect();
    // Its replacement reaches above the first struck line.
    assert!(rect[3] > quads(strike)[1], "{rect:?}");

    // What can't be found goes on a page of its own, which links to annox.
    let appendix = pdf_annots(&output, 2);
    let names: Vec<&str> = appendix.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["", gone.as_str()]);
    let uri = appendix[0].1.get(b"A").unwrap().as_dict().unwrap().get(b"URI").unwrap().as_str().unwrap();
    assert_eq!(uri, b"https://gaardhus.github.io/annox/");
    let doc = lopdf::Document::load(&output).unwrap();
    let text = doc.extract_text(&[2]).unwrap();
    assert!(text.contains("Review comments on paper.md, exported by annox"), "{text}");
}

/// The `/QuadPoints` of an annotation.
fn quads(d: &lopdf::Dictionary) -> Vec<f32> {
    d.get(b"QuadPoints").unwrap().as_array().unwrap().iter().map(|v| v.as_float().unwrap()).collect()
}

fn annot<'a>(annots: &'a [(String, lopdf::Dictionary)], nm: &str) -> &'a lopdf::Dictionary {
    &annots.iter().find(|(n, _)| n == nm).unwrap_or_else(|| panic!("no annotation {nm}")).1
}

#[test]
fn export_pdf_tells_repeated_quotes_apart_by_context() {
    let dir = setup("First, the bound is tight here.\nSecond, the bound is tight there.\n");
    let d = dir.path();
    write_pdf(&d.join("paper.pdf"), &[&["First, the bound is tight here.", "Second, the bound is tight there."]]);
    let second =
        id(&annox(d, &["comment", "paper.md", "--quote", "bound is tight", "--occurrence", "2", "--body", "b"]));
    annox(d, &["export-pdf", "paper.md", "paper.pdf"]);
    let q = quads(annot(&pdf_annots(&d.join("paper.annotated.pdf"), 1), &second));
    // The second line's baseline is at 680, the first's at 700.
    assert!(q[5] < 680.0 && q[1] > 680.0 && q[1] < 700.0, "{q:?}");
}

#[test]
fn export_pdf_continues_a_quote_onto_the_next_page() {
    let dir = setup("The bound is\ntight.\n");
    let d = dir.path();
    write_pdf(&d.join("paper.pdf"), &[&["The bound is"], &["tight."]]);
    let c = id(&annox(d, &["comment", "paper.md", "--quote", "bound is\ntight", "--body", "b"]));
    annox(d, &["export-pdf", "paper.md", "paper.pdf"]);
    let output = d.join("paper.annotated.pdf");
    let first = annot(&pdf_annots(&output, 1), &c).clone();
    assert_eq!(first.get(b"Contents").unwrap().as_str().unwrap(), b"b");
    let rest = annot(&pdf_annots(&output, 2), &format!("{c}-1")).clone();
    assert_eq!(rest.get(b"Subtype").unwrap().as_name().unwrap(), b"Highlight");
    assert!(rest.get(b"Contents").is_err());
}

#[test]
fn export_pdf_places_threads_on_text_gone_from_the_source() {
    // The PDF was compiled before the paragraph was deleted.
    let dir = setup("Keep this.\n\nThis paragraph goes away later.\n");
    let d = dir.path();
    write_pdf(&d.join("paper.pdf"), &[&["Keep this.", "This paragraph goes away later."]]);
    let c = id(&annox(d, &["comment", "paper.md", "--quote", "goes away", "--body", "b"]));
    std::fs::write(d.join("paper.md"), "Keep this.\n").unwrap();
    assert_eq!(annox(d, &["show", &c])["resolution"], "orphaned");
    let out = annox(d, &["export-pdf", "paper.md", "paper.pdf"]);
    assert_eq!(out["placed"], serde_json::json!([c]));
}

#[test]
fn export_pdf_takes_list_filters() {
    let dir = setup(DOC);
    let d = dir.path();
    write_pdf(&d.join("paper.pdf"), &[&["In Section 3, we prove that teh bound is tight.", "The bound is tight."]]);
    let typo = id(&annox(d, &["suggest", "paper.md", "--quote", "teh", "--replace", "the"]));
    let done = id(&annox(d, &["comment", "paper.md", "--quote", "Section 3", "--body", "b"]));
    annox(d, &["status", &done, "resolved"]);
    let ids = |args: &[&str]| annox(d, &[&["export-pdf", "paper.md", "paper.pdf"], args].concat())["placed"].clone();
    assert_eq!(ids(&[]), serde_json::json!([typo]));
    assert_eq!(ids(&["--closed"]), serde_json::json!([typo, done]));
    assert_eq!(ids(&["--closed", "--kind", "comment"]), serde_json::json!([done]));
}

#[test]
fn export_pdf_writes_to_output_and_keeps_existing_annotations() {
    let dir = setup(DOC);
    let d = dir.path();
    let input = d.join("paper.pdf");
    write_pdf(&input, &[&["In Section 3, we prove that teh bound is tight."]]);
    // An annotation already in the PDF, in an indirect /Annots array.
    use lopdf::dictionary;
    let mut doc = lopdf::Document::load(&input).unwrap();
    let link = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link", "NM" => lopdf::Object::string_literal("link"),
        "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
    });
    let array = doc.add_object(vec![lopdf::Object::Reference(link)]);
    let page = doc.get_pages()[&1];
    doc.get_object_mut(page).unwrap().as_dict_mut().unwrap().set("Annots", array);
    doc.save(&input).unwrap();
    let before = std::fs::read(&input).unwrap();

    let c = id(&annox(d, &["comment", "paper.md", "--quote", "teh", "--body", "typo"]));
    let out = annox(d, &["export-pdf", "paper.md", "paper.pdf", "-o", "review.pdf"]);
    assert_eq!(Path::new(out["output"].as_str().unwrap()), d.canonicalize().unwrap().join("review.pdf"));
    let annots = pdf_annots(&d.join("review.pdf"), 1);
    let names: Vec<&str> = annots.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["link", c.as_str()]);
    assert_eq!(std::fs::read(&input).unwrap(), before, "the input PDF is left alone");
}

#[test]
fn export_pdf_fails_on_what_isnt_a_pdf() {
    let dir = setup(DOC);
    let d = dir.path();
    assert!(error(d, &["export-pdf", "paper.md", "missing.pdf"]).contains("can't read"));
    std::fs::write(d.join("junk.pdf"), "not a pdf").unwrap();
    assert!(error(d, &["export-pdf", "paper.md", "junk.pdf"]).contains("can't read"));
}

/// Compiles real documents with typst and pdflatex, when they're installed.
#[test]
fn export_pdf_handles_typst_and_latex_output() {
    let have =
        |tool: &str| std::process::Command::new(tool).arg("--version").output().is_ok_and(|o| o.status.success());
    let body = "We prove that the BOUND is tight for all inputs, a substantially stronger statement than the \
                previous characterisation of incomprehensibilities in the literature.\n\n\
                The bound is tight. Again: the bound is tight.\n";
    let cases = [
        ("typst", "paper.typ", "#set page(width: 12cm, height: auto)\n= Intro\n", "#emph[bound]", ""),
        (
            "pdflatex",
            "paper.tex",
            "\\documentclass{article}\\usepackage[a6paper]{geometry}\\begin{document}\n",
            "\\emph{bound}",
            "\\end{document}\n",
        ),
    ];
    for (tool, file, head, bound, tail) in cases {
        if !have(tool) {
            eprintln!("skipping {file}: {tool} isn't installed");
            continue;
        }
        let dir = setup("");
        let d = dir.path();
        std::fs::write(d.join(file), format!("{head}{}{tail}", body.replace("BOUND", bound))).unwrap();
        let status = std::process::Command::new(tool)
            .args(if tool == "typst" {
                vec!["compile", file, "paper.pdf"]
            } else {
                vec!["-interaction=batchmode", file]
            })
            .current_dir(d)
            .output()
            .unwrap()
            .status;
        assert!(status.success(), "{tool} failed");
        let marked = id(&annox(d, &["comment", file, "--quote", &format!("{bound} is tight"), "--body", "markup"]));
        let broken =
            id(&annox(d, &["comment", file, "--quote", "characterisation of incomprehensibilities", "--body", "b"]));
        let last = id(&annox(d, &["suggest", file, "--quote", "the bound is tight", "--replace", "it is"]));
        let out = annox(d, &["export-pdf", file, "paper.pdf"]);
        assert_eq!(out["placed"], serde_json::json!([marked, broken, last]), "{file}");
        let annots = pdf_annots(&d.join("paper.annotated.pdf"), 1);
        // The last of four "the bound is tight"s, by its context: right of the
        // first on its line, which is the same line.
        let first = quads(annot(&annots, &marked));
        let q = quads(annot(&annots, &last));
        assert!(q[5] < first[5] && q[0] > 100.0, "{file}: {q:?}");
    }
}

#[test]
fn export_pdf_tints_the_words_a_suggestion_changes() {
    let dir = setup("We prove the old bound holds here.\n");
    let d = dir.path();
    write_pdf(&d.join("paper.pdf"), &[&["We prove the old bound holds here."]]);
    let s = id(&annox(
        d,
        &["suggest", "paper.md", "--quote", "prove the old bound holds", "--replace", "prove that the new bound holds"],
    ));
    annox(d, &["export-pdf", "paper.md", "paper.pdf"]);
    let annots = pdf_annots(&d.join("paper.annotated.pdf"), 1);
    // The whole quote is struck, as one quad.
    assert_eq!(quads(annot(&annots, &s)).len(), 8);
    let caret = annot(&annots, &format!("{s}-caret"));
    let doc = lopdf::Document::load(d.join("paper.annotated.pdf")).unwrap();
    let form = doc.get_object(caret.get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap());
    let ops = String::from_utf8(form.unwrap().as_stream().unwrap().content.clone()).unwrap();
    // The whole replacement is written above it. "old" is tinted, and so are
    // "that" and "new" in the replacement: three boxes, multiplied over the text.
    assert!(ops.contains("(prove that the new bound holds) Tj"), "{ops}");
    assert_eq!(ops.matches(" re f").count(), 3, "{ops}");
    assert!(ops.contains("/Multiply gs"), "{ops}");
}

#[test]
fn export_pdf_notes_a_replacement_only_when_it_was_cut_short() {
    let dir = setup("Short text here. And more.\n");
    let d = dir.path();
    write_pdf(&d.join("paper.pdf"), &[&["Short text here. And more."]]);
    let long = "a much longer replacement ".repeat(12);
    let cut = id(&annox(d, &["suggest", "paper.md", "--quote", "here", "--replace", long.trim(), "--body", "Why"]));
    let full = id(&annox(d, &["suggest", "paper.md", "--quote", "more", "--replace", "less", "--body", "Why"]));
    annox(d, &["export-pdf", "paper.md", "paper.pdf"]);
    let annots = pdf_annots(&d.join("paper.annotated.pdf"), 1);
    // A PDF text string: UTF-16BE after a byte order mark, or else ASCII here.
    let note = |id: &str| {
        let bytes = annot(&annots, id).get(b"Contents").unwrap().as_str().unwrap().to_vec();
        match bytes.strip_prefix(&[0xFE, 0xFF]) {
            Some(utf16) => {
                String::from_utf16(&utf16.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>())
                    .unwrap()
            }
            None => String::from_utf8(bytes).unwrap(),
        }
    };
    assert_eq!(note(&full), "Why");
    assert!(note(&cut).contains("a much longer replacement") && note(&cut).ends_with("Why"), "{}", note(&cut));
}

/// The width of a character of 12 pt Courier.
const COURIER: f32 = 7.2;

/// The box around characters `from..to` of line `line` of a page that
/// [`write_pdf_in`] wrote in Courier, as `[x0, y0, x1, y1]`.
fn courier_box(line: usize, from: usize, to: usize) -> [f32; 4] {
    let y = 700.0 - 20.0 * line as f32;
    [72.0 + COURIER * from as f32, y - 3.0, 72.0 + COURIER * to as f32, y + 10.0]
}

/// Adds the annotation `annot` to page 1 of the PDF at `path`, as a viewer
/// would, and returns its object id.
fn add_annot(path: &Path, mut annot: lopdf::Dictionary) -> lopdf::ObjectId {
    use lopdf::Object;
    let mut doc = lopdf::Document::load(path).unwrap();
    let page = doc.get_pages()[&1];
    annot.set("Type", Object::Name(b"Annot".to_vec()));
    annot.set("P", page);
    let id = doc.add_object(annot);
    let page = doc.get_object_mut(page).unwrap().as_dict_mut().unwrap();
    let mut annots = page.get(b"Annots").ok().and_then(|a| a.as_array().ok().cloned()).unwrap_or_default();
    annots.push(id.into());
    page.set("Annots", annots);
    doc.save(path).unwrap();
    id
}

/// A viewer's markup annotation over `boxes`, by Jane at `date`.
fn markup(subtype: &str, nm: &str, contents: &str, boxes: &[[f32; 4]]) -> lopdf::Dictionary {
    use lopdf::Object;
    let quads: Vec<Object> =
        boxes.iter().flat_map(|[x0, y0, x1, y1]| [*x0, *y1, *x1, *y1, *x0, *y0, *x1, *y0]).map(Object::Real).collect();
    let mut d = note("Text", nm, contents);
    d.set("Subtype", Object::Name(subtype.as_bytes().to_vec()));
    d.set("Rect", boxes[0].iter().map(|&v| Object::Real(v)).collect::<Vec<_>>());
    d.set("QuadPoints", quads);
    d
}

/// A viewer's annotation of `subtype` with no markup, by Jane.
fn note(subtype: &str, nm: &str, contents: &str) -> lopdf::Dictionary {
    use lopdf::{Object, StringFormat};
    let s = |v: &str| Object::String(v.as_bytes().to_vec(), StringFormat::Literal);
    let mut d = lopdf::Dictionary::new();
    d.set("Subtype", Object::Name(subtype.as_bytes().to_vec()));
    d.set("NM", s(nm));
    d.set("T", s("Jane Doe"));
    d.set("Contents", s(contents));
    d.set("CreationDate", s("D:20261020120000+02'00'"));
    d.set("Rect", vec![Object::Real(0.0), Object::Real(0.0), Object::Real(0.0), Object::Real(0.0)]);
    d
}

#[test]
fn annotates_a_pdf_as_it_is() {
    let dir = setup("");
    let d = dir.path();
    write_pdf(&d.join("paper.pdf"), &[&["The bound is tight. The bound is", "loose."], &["Appendix"]]);
    // A line per line of text, and a blank line between pages.
    assert_eq!(annox(d, &["text", "paper.pdf"]), "The bound is tight. The bound is\nloose.\n\nAppendix\n");
    // Quotes match across line breaks, as copied from a viewer.
    let loose = annox(d, &["suggest", "paper.pdf", "--quote", "bound is loose.", "--replace", "bound is slack."]);
    assert_eq!(loose["line"], 1);
    let tight = annox(d, &["comment", "paper.pdf", "--quote", "tight", "--body", "Prove it"]);
    let list = annox(d, &["list", "paper.pdf"]);
    assert_eq!(list[0]["quote"], "bound is\nloose.");
    assert_eq!(list[1]["resolution"], "exact");
    assert!(error(d, &["accept", &id(&loose)]).contains("is a PDF"));

    // It exports into itself.
    let out = annox(d, &["export-pdf", "paper.pdf"]);
    assert_eq!(out["placed"], serde_json::json!([id(&loose), id(&tight)]));
    assert!(d.join("paper.annotated.pdf").exists());
    assert!(error(d, &["export-pdf", "paper.md"]).contains("pass the PDF"));
}

#[test]
fn import_pdf_adds_what_reviewers_wrote_in_a_viewer() {
    use lopdf::Object;
    let dir = setup("# Title\n\nThe bound is *tight*, and\nthe proof is short.\n\nIt holds for all n.\n");
    let d = dir.path();
    let pdf = d.join("paper.pdf");
    let lines: &[&str] =
        &["Title", "The bound is tight, and the", "proof is short.", "It holds for all n.", "Page 1 of 1"];
    write_pdf_in(&pdf, &[lines], "Courier");
    let mine = id(&annox(d, &["comment", "paper.md", "--quote", "It holds", "--body", "For all?"]));
    annox(d, &["export-pdf", "paper.md", "paper.pdf", "-o", "paper.pdf"]);

    // A highlight across two lines, with a reply to it.
    let highlight = add_annot(&pdf, markup("Highlight", "h1", "Why?", &[courier_box(1, 20, 27), courier_box(2, 0, 5)]));
    let mut reply = note("Text", "r1", "Because.");
    reply.set("IRT", highlight);
    add_annot(&pdf, reply);
    // "Replace text": struck text, and a caret grouped with it.
    let struck = add_annot(&pdf, markup("StrikeOut", "s1", "Too strong", &[courier_box(1, 13, 18)]));
    let mut caret = markup("Caret", "c1", "close", &[courier_box(1, 18, 18)]);
    caret.set("IRT", struck);
    caret.set("RT", Object::Name(b"Group".to_vec()));
    add_annot(&pdf, caret);
    // A reply to the comment annox exported, and a review state, left out.
    let doc = lopdf::Document::load(&pdf).unwrap();
    let annots = doc.get_dictionary(doc.get_pages()[&1]).unwrap().get(b"Annots").unwrap().as_array().unwrap().clone();
    let exported = pdf_annots(&pdf, 1).iter().position(|(nm, _)| *nm == mine).unwrap();
    let exported = annots[exported].as_reference().unwrap();
    let mut answer = note("Text", "r2", "Yes, all.");
    answer.set("IRT", exported);
    add_annot(&pdf, answer);
    let mut state = note("Text", "st", "Accepted");
    state.set("IRT", exported);
    state.set("State", lopdf::Object::String(b"Accepted".to_vec(), lopdf::StringFormat::Literal));
    add_annot(&pdf, state);
    // A highlight on text that isn't in the source.
    add_annot(&pdf, markup("Highlight", "h2", "Footer?", &[courier_box(4, 0, 4)]));

    let dry = annox(d, &["import-pdf", "paper.md", "paper.pdf", "--dry-run"]);
    assert_eq!(dry["dryRun"], true);
    assert_eq!(annox(d, &["list", "paper.md"]).as_array().unwrap().len(), 1);

    let out = annox(d, &["import-pdf", "paper.md", "paper.pdf"]);
    assert_eq!(out["unchanged"], 1, "{out:#}");
    let added = out["added"].as_array().unwrap();
    let kinds: Vec<&str> = added.iter().map(|a| a["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["comment", "suggestion", "reply", "reply"], "{out:#}");
    assert_eq!(added[0]["quote"], "and\nthe proof");
    // Struck text is placed on the source's words, inside their markup.
    assert_eq!(added[1]["quote"], "tight");
    assert_eq!(added[1]["replacement"], "close");
    assert_eq!(added[3]["parent"], mine.as_str());
    assert_eq!(out["skipped"][0]["quote"], "Page");
    assert_eq!(out["skipped"][0]["reason"], "its text isn't in paper.md");

    let list = annox(d, &["list", "paper.md"]);
    let why = list.as_array().unwrap().iter().find(|a| a["body"] == "Why?").unwrap();
    assert_eq!(why["author"], serde_json::json!({ "id": "urn:annox:pdf:Jane%20Doe", "name": "Jane Doe" }));
    assert_eq!(why["created"], "2026-10-20T10:00:00Z");
    assert_eq!(why["replies"][0]["body"], "Because.");

    // Importing again adds nothing.
    let again = annox(d, &["import-pdf", "paper.md", "paper.pdf"]);
    assert_eq!(again["added"], serde_json::json!([]));

    // Importing into the PDF itself adds the PDF's own comments there too,
    // but not the reply already added to the source's thread.
    let into_pdf = annox(d, &["import-pdf", "paper.pdf"]);
    let kinds: Vec<&str> = into_pdf["added"].as_array().unwrap().iter().map(|a| a["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["comment", "suggestion", "comment", "reply"], "{into_pdf:#}");
    assert_eq!(into_pdf["unchanged"], 2, "{into_pdf:#}");
    assert_eq!(annox(d, &["import-pdf", "paper.pdf"])["added"], serde_json::json!([]));
}
