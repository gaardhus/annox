//! Checks that this build still reads the committed example in `examples/`.
//!
//! The example was written by an older annox. If this fails after a format
//! change, regenerate the example rather than loosening the test.

use std::path::Path;

use annox_lsp::cli;
use serde_json::{json, Value};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn annox(cwd: &Path, args: &[&str]) -> Value {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    cli::run(&args, cwd).unwrap()
}

#[test]
fn the_example_still_reads_the_same() {
    // A copy, so that reading it never writes caches into the repository.
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples"), dir.path());

    let report = annox(dir.path(), &["report", "--json"]);
    let doc = &report["documents"][0];
    assert_eq!(report["documents"].as_array().unwrap().len(), 1, "{report:#}");
    assert_eq!(doc["path"], "proposal.md");
    assert_eq!(doc["comments"], json!({ "open": 3, "resolved": 1 }), "{doc:#}");
    assert_eq!(doc["suggestions"], json!({ "accepted": 1, "open": 2, "rejected": 1, "withdrawn": 0 }), "{doc:#}");
    for broken in ["orphaned", "stale", "conflicted"] {
        assert_eq!(doc[broken], 0, "{broken}: {doc:#}");
    }
    assert_eq!(doc["missing"], false);

    let all = annox(dir.path(), &["list", "--all"]);
    let all = all.as_array().unwrap();
    for a in all.iter().filter(|a| a["status"] == "open") {
        assert_ne!(a["resolution"], "orphaned", "{a:#}");
        if a["kind"] == "suggestion" {
            assert_eq!(a["applicable"], true, "{a:#}");
        }
    }
    assert!(all.iter().any(|a| a["body"].is_null()), "a highlight");
    assert_eq!(all.iter().map(|a| a["replies"].as_array().unwrap().len()).sum::<usize>(), 3, "replies");
    let authors: std::collections::BTreeSet<_> = all.iter().map(|a| a["author"]["id"].as_str().unwrap()).collect();
    assert_eq!(authors.len(), 2, "{authors:?}");
}
