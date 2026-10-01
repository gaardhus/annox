use std::net::TcpListener;
use std::path::PathBuf;

use annox_sync::hub::{serve, HubConfig};
use clap::{Parser, Subcommand};
use lsp_server::Connection;

/// Review comments and suggested edits, stored next to your files in `.annox/`.
///
/// The annotation commands print JSON, except `report` without --json. Their
/// write commands take --author and --name, or read ANNOX_AUTHOR and
/// ANNOX_AUTHOR_NAME, then the author in ~/.config/annox/config.json, and
/// otherwise use git's identity.
#[derive(Parser)]
#[command(name = "annox", version, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(flatten)]
    Annotations(annox_lsp::cli::Command),
    /// Run the language server over stdio (spec §6)
    Lsp,
    /// Run a sync hub for sharing workspaces (spec §7)
    Hub {
        /// Where the hub keeps its workspaces
        #[arg(long, value_name = "DIR")]
        data: PathBuf,
        /// The address to listen on
        #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:7878")]
        listen: String,
        /// A file holding the token clients must present
        #[arg(long, value_name = "FILE")]
        token_file: Option<PathBuf>,
    },
    /// Run an MCP server over stdio, for agents
    Mcp {
        /// The workspace to serve [default: the current directory]
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// Author id for the agent's annotations
        #[arg(long, value_name = "ID")]
        author: Option<String>,
        /// Display name for the agent's annotations
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
    },
    /// Install the latest release over this binary, and the Claude Code skill if it's installed
    ///
    /// Runs the release's install.sh, so it needs curl and sh.
    Update {
        /// Install this release instead of the latest
        #[arg(long, value_name = "vX.Y.Z")]
        version: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Lsp => {
            let (connection, io_threads) = Connection::stdio();
            annox_lsp::run(&connection)?;
            drop(connection);
            io_threads.join()?;
            Ok(())
        }
        Command::Hub { data, listen, token_file } => {
            // A file, not a flag value, so the token doesn't show up in `ps`.
            let token = token_file.map(std::fs::read_to_string).transpose()?.map(|t| t.trim().to_owned());
            let listener = TcpListener::bind(&listen)?;
            eprintln!("annox hub listening on ws://{} (workspaces at /w/<name>)", listener.local_addr()?);
            serve(listener, HubConfig { data, token })?;
            Ok(())
        }
        Command::Mcp { root, author, name } => {
            let root = root.map_or_else(std::env::current_dir, Ok)?;
            let config = annox_lsp::mcp::Config { root, author, name };
            annox_lsp::mcp::serve(std::io::stdin().lock(), std::io::stdout().lock(), &config)?;
            Ok(())
        }
        Command::Annotations(command) => {
            let text = matches!(command, annox_lsp::cli::Command::Report { json: false, .. });
            match annox_lsp::cli::execute(command, &std::env::current_dir()?) {
                Ok(output) if text => {
                    print!("{}", annox_lsp::cli::render_report(&output));
                    Ok(())
                }
                Ok(output) => {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                    Ok(())
                }
                Err(e) => {
                    eprintln!("annox: {e:#}");
                    std::process::exit(1);
                }
            }
        }
        Command::Update { version } => update(version),
    }
}

const REPO: &str = "gaardhus/annox";

/// Reinstalls this binary with the release's own `install.sh`, so the download and checksum logic
/// lives in one place.
fn update(version: Option<String>) -> anyhow::Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    if cfg!(windows) {
        anyhow::bail!(
            "annox update doesn't support Windows; download the .zip from https://github.com/{REPO}/releases"
        );
    }
    let current = concat!("v", env!("CARGO_PKG_VERSION"));
    let version = match version {
        Some(v) => format!("v{}", v.trim_start_matches('v')),
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
    let mut sh = std::process::Command::new("sh").arg("-s").arg("--").args(&install).stdin(Stdio::piped()).spawn()?;
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

#[cfg(test)]
mod tests {
    #[test]
    fn cli_is_well_formed() {
        use clap::CommandFactory;
        super::Cli::command().debug_assert();
    }
}
