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
    /// Install the latest release over this binary, and the agent skill wherever it's installed
    ///
    /// Runs the release's install.sh, so it needs curl and sh; on Windows, its install.ps1, with
    /// curl and PowerShell.
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
            let render: Option<fn(&serde_json::Value) -> String> = match command {
                annox_lsp::cli::Command::Report { json: false, verbose: false, .. } => {
                    Some(annox_lsp::cli::render_report)
                }
                annox_lsp::cli::Command::Report { verbose: true, .. } => Some(annox_lsp::cli::render_report_verbose),
                annox_lsp::cli::Command::History { json: false, .. } => Some(annox_lsp::cli::render_history),
                _ => None,
            };
            match (annox_lsp::cli::execute(command, &std::env::current_dir()?), render) {
                (Ok(output), Some(render)) => {
                    print!("{}", render(&output));
                    Ok(())
                }
                (Ok(output), None) => {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                    Ok(())
                }
                (Err(e), _) => {
                    eprintln!("annox: {e:#}");
                    std::process::exit(1);
                }
            }
        }
        Command::Update { version } => update(version),
    }
}

const REPO: &str = "gaardhus/annox";

/// Reinstalls this binary with the release's own `install.sh`, or `install.ps1` on Windows, so the
/// download and checksum logic lives in one place.
fn update(version: Option<String>) -> anyhow::Result<()> {
    let current = concat!("v", env!("CARGO_PKG_VERSION"));
    let version = match version {
        Some(v) => format!("v{}", v.trim_start_matches('v')),
        None => {
            // The latest release page redirects to its tag.
            let url = curl(&[
                "-fsSLI",
                "-o",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
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
    let skills = installed_skills();

    eprintln!("annox: updating {current} -> {version}");
    let ok = if cfg!(windows) { update_windows(&version, dir, skills)? } else { update_unix(&version, dir, skills)? };
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

/// Where the installer should put the skill, to update the copies that are already installed.
enum Skills {
    None,
    /// Both of the installer's default directories.
    Defaults,
    Only(PathBuf),
}

fn installed_skills() -> Skills {
    let has = |d: &PathBuf| d.join("annox").is_dir();
    if let Ok(d) = std::env::var("ANNOX_SKILL_DIR") {
        if !d.is_empty() {
            let d = PathBuf::from(d);
            return if has(&d) { Skills::Only(d) } else { Skills::None };
        }
    }
    let Some(home) = std::env::home_dir() else { return Skills::None };
    // The installer's defaults: Claude Code's directory, and the one most other agents share.
    let claude = home.join(".claude").join("skills");
    let agents = home.join(".agents").join("skills");
    match (has(&claude), has(&agents)) {
        (true, true) => Skills::Defaults,
        (true, false) => Skills::Only(claude),
        (false, true) => Skills::Only(agents),
        (false, false) => Skills::None,
    }
}

fn update_unix(version: &str, dir: &std::path::Path, skills: Skills) -> anyhow::Result<bool> {
    use std::io::Write;
    use std::process::Stdio;

    let mut install = vec!["--version".to_owned(), version.to_owned(), "--dir".to_owned(), dir.display().to_string()];
    match skills {
        Skills::None => {}
        Skills::Defaults => install.push("--skill".to_owned()),
        Skills::Only(d) => install.extend(["--skill".to_owned(), "--skill-dir".to_owned(), d.display().to_string()]),
    }
    let script = curl(&["-fsSL", &format!("https://raw.githubusercontent.com/{REPO}/{version}/install.sh")])?;
    let mut sh = std::process::Command::new("sh").arg("-s").arg("--").args(&install).stdin(Stdio::piped()).spawn()?;
    sh.stdin.take().expect("piped stdin").write_all(&script)?;
    Ok(sh.wait()?.success())
}

/// install.ps1 renames the running annox.exe out of the way, since Windows won't overwrite it, and
/// leaves it behind as annox.exe.old for the next update to remove.
fn update_windows(version: &str, dir: &std::path::Path, skills: Skills) -> anyhow::Result<bool> {
    let url = format!("https://raw.githubusercontent.com/{REPO}/{version}/install.ps1");
    let script = curl(&["-fsSL", &url]).map_err(|e| {
        anyhow::anyhow!(
            "{e}\n{version} may predate Windows support; download the .zip from https://github.com/{REPO}/releases"
        )
    })?;
    // PowerShell only takes parameters for a script it runs from a file.
    let file = std::env::temp_dir().join(format!("annox-install-{}.ps1", std::process::id()));
    std::fs::write(&file, script)?;
    let mut ps = std::process::Command::new("powershell");
    ps.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&file);
    ps.arg("-Version").arg(version).arg("-Dir").arg(dir).arg("-NoModifyPath");
    match skills {
        Skills::None => {}
        Skills::Defaults => {
            ps.arg("-Skill");
        }
        Skills::Only(d) => {
            ps.arg("-Skill").arg("-SkillDir").arg(d);
        }
    }
    let status = ps.status();
    let _ = std::fs::remove_file(&file);
    Ok(status.map_err(|e| anyhow::anyhow!("annox update needs PowerShell: {e}"))?.success())
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
