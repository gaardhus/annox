//! Runs the examples in the README's "Command line" section, so they can't go stale.
//!
//! The blocks run in order in one workspace, and `COMMENT_ID` and `SUGGESTION_ID` stand
//! for the ids of the last comment and suggestion made. If this fails after a change to
//! the CLI, fix the README rather than the test.

use std::path::Path;

use annox_lsp::cli;

/// Has the text the examples quote.
const DOC: &str = "We prove that the bound is tight; see teh appendix.\n";

/// The `annox` lines of the `sh` blocks between the command listing and the next
/// section, without their trailing comments.
fn examples(readme: &str) -> Vec<String> {
    let start = readme.find("<!-- /annox -h -->").expect("README has no `annox -h` block");
    let section = &readme[start..];
    let section = &section[..section.find("\n### ").unwrap_or(section.len())];
    let mut lines = Vec::new();
    let mut in_sh = false;
    for line in section.lines() {
        match line {
            "```sh" => in_sh = true,
            "```" => in_sh = false,
            _ if in_sh && line.starts_with("annox ") => {
                lines.push(line.split("  #").next().unwrap().trim_end().to_owned());
            }
            _ => {}
        }
    }
    lines
}

#[test]
fn the_readme_examples_run() {
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md")).unwrap();
    let lines = examples(&readme);
    assert!(lines.len() > 5, "found only {lines:?} in the README");

    let dir = tempfile::tempdir().unwrap();
    // `annox commit` needs a repository.
    let git = std::process::Command::new("git").arg("init").arg("-q").current_dir(dir.path()).status().unwrap();
    assert!(git.success());
    std::fs::write(dir.path().join("paper.md"), DOC).unwrap();
    cli::run(&["init".into()], dir.path()).unwrap();

    let (mut comment, mut suggestion) = (String::new(), String::new());
    for line in &lines {
        let words = shlex::split(line).unwrap_or_else(|| panic!("can't split {line:?}"));
        let mut args: Vec<String> = words[1..]
            .iter()
            // `log --follow` never returns; the rest of the command still has to work.
            .filter(|w| *w != "--follow")
            .map(|w| match w.as_str() {
                "COMMENT_ID" => comment.clone(),
                "SUGGESTION_ID" => suggestion.clone(),
                _ => w.clone(),
            })
            .collect();
        if !matches!(args[0].as_str(), "list" | "show" | "history" | "log" | "report" | "commit") {
            args.extend(["--author".into(), "urn:test:reader".into()]);
        }
        let out = cli::run(&args, dir.path()).unwrap_or_else(|e| panic!("`{line}` failed: {e:#}"));
        match args[0].as_str() {
            "comment" => comment = out["id"].as_str().unwrap().to_owned(),
            "suggest" => suggestion = out["id"].as_str().unwrap().to_owned(),
            _ => {}
        }
    }
}
