//! The per-user config file. One test in its own binary, since it sets
//! process-wide environment variables.

use annox_lsp::{cli, default_author};
use serde_json::json;

#[test]
fn config_file_sets_the_default_author() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.json");
    // SAFETY: this binary runs no other test, so nothing reads the environment concurrently.
    unsafe {
        std::env::set_var("ANNOX_CONFIG_FILE", &config);
        std::env::set_var("ANNOX_CREDENTIALS_FILE", home.path().join("credentials.json"));
        std::env::remove_var("ANNOX_OUTPUT");
        std::env::remove_var("ANNOX_AUTHOR");
        std::env::remove_var("ANNOX_AUTHOR_NAME");
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("paper.md"), "Some text.\n").unwrap();
    let run = |args: &[&str]| cli::run(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), dir.path());
    run(&["init"]).unwrap();

    std::fs::write(&config, json!({ "author": { "id": "mailto:ada@example.org", "name": "Ada" } }).to_string())
        .unwrap();
    assert_eq!(default_author(dir.path()).unwrap(), json!({ "id": "mailto:ada@example.org", "name": "Ada" }));

    // `annox config` says where it comes from, and what the files hold.
    let shown = run(&["config"]).unwrap();
    assert_eq!(shown["author"]["from"], json!({ "id": "config", "name": "config" }), "{shown}");
    assert_eq!(shown["output"], json!({ "value": "text", "from": "default" }));
    assert_eq!(shown["workspace"]["format"], json!(1));
    assert_eq!(shown["workspace"]["sync"], json!(null));
    let states: Vec<_> = shown["files"].as_array().unwrap().iter().map(|f| f["state"].clone()).collect();
    assert_eq!(states, [json!("ok"), json!("missing"), json!("ok")]);

    // The CLI writes as the configured author, and --name only renames it.
    run(&["comment", "paper.md", "--quote", "Some", "--body", "a"]).unwrap();
    run(&["comment", "paper.md", "--quote", "text", "--body", "b", "--name", "A. L."]).unwrap();
    // An explicit id is a different identity and doesn't borrow the configured name.
    unsafe { std::env::set_var("ANNOX_AUTHOR", "urn:agent:claude") };
    run(&["comment", "paper.md", "--quote", "Some text", "--body", "c"]).unwrap();
    unsafe { std::env::remove_var("ANNOX_AUTHOR") };
    let list = run(&["list", "paper.md"]).unwrap();
    let authors: Vec<_> = list.as_array().unwrap().iter().map(|a| (a["body"].clone(), a["author"].clone())).collect();
    assert!(authors.contains(&(json!("a"), json!({ "id": "mailto:ada@example.org", "name": "Ada" }))), "{authors:?}");
    assert!(authors.contains(&(json!("b"), json!({ "id": "mailto:ada@example.org", "name": "A. L." }))), "{authors:?}");
    assert!(authors.contains(&(json!("c"), json!({ "id": "urn:agent:claude" }))), "{authors:?}");

    // A broken file is an error, naming the file, rather than silently ignored.
    std::fs::write(&config, r#"{ "author": { "name": "Ada" } }"#).unwrap();
    let err = format!("{:#}", run(&["comment", "paper.md", "--quote", "Some", "--body", "d"]).unwrap_err());
    assert!(err.contains("config.json") && err.contains("author.id"), "{err}");
    // `annox config` reports it instead of failing.
    let shown = run(&["config"]).unwrap();
    assert_eq!(shown["author"], json!(null));
    assert_eq!(shown["files"][0]["state"], json!("invalid"));
    assert!(shown["files"][0]["error"].as_str().unwrap().starts_with("`author.id`"), "{shown}");
}
