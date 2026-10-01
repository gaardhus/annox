//! Tests of the command-line interface, run in-process.

use std::path::Path;

use annox_lsp::cli;
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
    if !matches!(args[0].as_str(), "init" | "list" | "show" | "report") {
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

    annox(dir.path(), &["edit", &c, "--body", "Which section?", "--label", "question"]);
    let all = annox(dir.path(), &["list", "--all"]);
    assert_eq!((&all[0]["body"], &all[0]["label"]), (&"Which section?".into(), &"question".into()));
    annox(dir.path(), &["edit", &c, "--label", ""]);
    assert_eq!(annox(dir.path(), &["list", "--all"])[0]["label"], Value::Null);

    annox(dir.path(), &["delete", &c]);
    assert_eq!(annox(dir.path(), &["list", "--all"])[0]["deleted"], true);
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

    let text = cli::render_report(&report);
    assert_eq!(
        text,
        "notes.md  file missing · 1 open comment · 1 orphaned\npaper.md  1 open comment · 1 open suggestion · 1 closed\n"
    );
    assert_eq!(cli::render_report(&serde_json::json!({ "documents": [] })), "no annotations\n");
}
