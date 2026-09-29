use std::net::TcpListener;
use std::path::PathBuf;

use annox_sync::hub::{serve, HubConfig};
use lsp_server::Connection;

const USAGE: &str = "usage:
  annox lsp                                   run the language server over stdio (spec §6)
  annox hub --data DIR [--listen ADDR] [--token-file FILE]
                                              run a sync hub (spec §7); ADDR defaults to 127.0.0.1:7878
  annox mcp [--root DIR] [--author ID] [--name NAME]
                                              run an MCP server over stdio, for agents; DIR defaults to
                                              the current directory
  annox update [--version vX.Y.Z]             install the latest (or given) release over this binary, and
                                              the Claude Code skill if it's installed; needs curl and sh
  annox --version                             print the version
";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("lsp") => {
            let (connection, io_threads) = Connection::stdio();
            annox_lsp::run(&connection)?;
            drop(connection);
            io_threads.join()?;
            Ok(())
        }
        Some("hub") => hub(&args[1..]),
        Some("mcp") => mcp(&args[1..]),
        Some("update") => update(&args[1..]),
        Some("--version" | "-V") => {
            println!("annox {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}{}", annox_lsp::cli::USAGE);
            Ok(())
        }
        Some(_) => match annox_lsp::cli::run(&args, &std::env::current_dir()?) {
            Ok(output) => {
                println!("{}", serde_json::to_string_pretty(&output)?);
                Ok(())
            }
            Err(e) => {
                eprintln!("annox: {e:#}");
                std::process::exit(1);
            }
        },
        None => {
            eprintln!("{USAGE}{}", annox_lsp::cli::USAGE);
            std::process::exit(2);
        }
    }
}

fn hub(args: &[String]) -> anyhow::Result<()> {
    let mut listen = "127.0.0.1:7878".to_owned();
    let mut data = None;
    let mut token = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| anyhow::anyhow!("{flag} needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--listen" => listen = value.clone(),
            "--data" => data = Some(PathBuf::from(value)),
            // A file, not a flag value, so the token doesn't show up in `ps`.
            "--token-file" => token = Some(std::fs::read_to_string(value)?.trim().to_owned()),
            _ => anyhow::bail!("unknown option {flag}\n{USAGE}"),
        }
    }
    let data = data.ok_or_else(|| anyhow::anyhow!("--data is required\n{USAGE}"))?;
    let listener = TcpListener::bind(&listen)?;
    eprintln!("annox hub listening on ws://{} (workspaces at /w/<name>)", listener.local_addr()?);
    serve(listener, HubConfig { data, token })?;
    Ok(())
}

fn mcp(args: &[String]) -> anyhow::Result<()> {
    let mut config = annox_lsp::mcp::Config { root: std::env::current_dir()?, author: None, name: None };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| anyhow::anyhow!("{flag} needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--root" => config.root = PathBuf::from(value),
            "--author" => config.author = Some(value.clone()),
            "--name" => config.name = Some(value.clone()),
            _ => anyhow::bail!("unknown option {flag}\n{USAGE}"),
        }
    }
    annox_lsp::mcp::serve(std::io::stdin().lock(), std::io::stdout().lock(), &config)?;
    Ok(())
}

const REPO: &str = "gaardhus/annox";

/// Reinstalls this binary with the release's own `install.sh`, so the download and checksum logic
/// lives in one place.
fn update(args: &[String]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if cfg!(windows) {
        anyhow::bail!(
            "annox update doesn't support Windows; download the .zip from https://github.com/{REPO}/releases"
        );
    }
    let mut version = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| anyhow::anyhow!("{flag} needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--version" => version = Some(format!("v{}", value.trim_start_matches('v'))),
            _ => anyhow::bail!("unknown option {flag}\n{USAGE}"),
        }
    }
    let current = concat!("v", env!("CARGO_PKG_VERSION"));
    let version = match version {
        Some(v) => v,
        None => {
            // The latest release page redirects to its tag.
            let url = curl(&[
                "-fsSLI",
                "-o",
                "/dev/null",
                "-w",
                "%{url_effective}",
                &format!("https://github.com/{REPO}/releases/latest"),
            ])?;
            let latest = String::from_utf8(url)?.rsplit('/').next().unwrap_or_default().to_owned();
            if latest == current {
                eprintln!("annox: already up to date ({current})");
                return Ok(());
            }
            latest
        }
    };

    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| anyhow::anyhow!("can't tell which directory {} is in", exe.display()))?;
    let mut install = vec!["--version".to_owned(), version.clone(), "--dir".to_owned(), dir.display().to_string()];
    let skills = match std::env::var("ANNOX_SKILL_DIR") {
        Ok(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude/skills")),
    };
    if let Some(skills) = skills.filter(|d| d.join("annox").is_dir()) {
        install.extend(["--skill".to_owned(), "--skill-dir".to_owned(), skills.display().to_string()]);
    }

    let script = curl(&["-fsSL", &format!("https://raw.githubusercontent.com/{REPO}/{version}/install.sh")])?;
    eprintln!("annox: updating {current} -> {version}");
    let mut sh = Command::new("sh").arg("-s").arg("--").args(&install).stdin(Stdio::piped()).spawn()?;
    sh.stdin.take().expect("piped stdin").write_all(&script)?;
    if !sh.wait()?.success() {
        std::process::exit(1);
    }
    Ok(())
}

fn curl(args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let out = std::process::Command::new("curl")
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("annox update needs curl: {e}"))?;
    if !out.status.success() {
        anyhow::bail!("curl {}: {}", args.last().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}
