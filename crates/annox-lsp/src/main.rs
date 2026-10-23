use std::net::TcpListener;
use std::path::PathBuf;

use annox_sync::hub::{serve, HubConfig};
use clap::{Parser, Subcommand};
use lsp_server::Connection;

/// Review comments and suggested edits, stored next to your files in `.annox/`.
///
/// The commands print text, or JSON with --json, or when ANNOX_OUTPUT=json or
/// "output": "json" in ~/.config/annox/config.json. The write commands take
/// --author and --name, or read ANNOX_AUTHOR and ANNOX_AUTHOR_NAME, then the
/// author in that file, and otherwise use git's identity.
#[derive(Parser)]
#[command(
    name = "annox",
    version,
    styles = annox_lsp::style::HELP,
    before_long_help = annox_lsp::style::banner()
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Print JSON instead of text [default: text, unless ANNOX_OUTPUT or the config file say json]
    #[arg(long, global = true)]
    json: bool,
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
        /// Also print the events each connection pushes and pulls; connections, and what's refused,
        /// are always printed
        #[arg(short, long)]
        verbose: bool,
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
        /// Also act as a Claude Code channel: push threads others changed for the agent into the session
        #[arg(long)]
        channel: bool,
        /// With --channel, only threads containing TEXT, such as "@claude", or that the agent wrote in
        /// (repeatable)
        #[arg(long, value_name = "TEXT", requires = "channel")]
        mention: Vec<String>,
        /// With --channel, how long a thread must stay unchanged before it's pushed, in milliseconds
        #[arg(long, value_name = "MS", default_value_t = 2000, requires = "channel")]
        settle: u64,
    },
    /// Print threads others changed for you as they change, until stopped
    ///
    /// Each is a thread as `annox show` prints it, or with --json, one line of JSON as `annox list`
    /// prints it. Threads already unread are printed at the start. A thread is printed again when
    /// someone else changes it; `annox read` stops that.
    Watch(annox_lsp::cli::Watch),
    /// Install the latest release over this binary, and the agent skill wherever it's installed
    ///
    /// Runs the release's install.sh, so it needs curl and sh; on Windows, its install.ps1, with
    /// curl and PowerShell.
    Update {
        /// Install this release instead of the latest
        #[arg(long, value_name = "vX.Y.Z")]
        version: Option<String>,
    },
    /// Write the man pages, annox.1 and one per subcommand, into DIR, or print annox.1
    ///
    /// The installer writes them next to the binary, in ../share/man/man1, where `man` looks for the
    /// pages of programs on the PATH.
    #[command(hide = true)]
    Man {
        /// The directory to write the pages into, such as ~/.local/share/man/man1
        dir: Option<PathBuf>,
    },
}

/// The subcommands under each heading of `annox --help`, in order.
const GROUPS: &[(&str, &[&str])] = &[
    ("Read", &["list", "show", "report", "history", "log", "watch", "text"]),
    ("Annotate", &["comment", "highlight", "suggest", "reply", "edit"]),
    ("Review", &["status", "react", "accept", "revert", "retarget", "reattach", "delete", "restore", "read"]),
    ("Workspace", &["init", "commit"]),
    ("PDF", &["export-pdf", "import-pdf"]),
    ("Servers", &["lsp", "hub", "mcp"]),
    ("Other", &["update"]),
];

/// `annox`'s command, with its subcommands listed under the headings of [`GROUPS`]: clap can't
/// group subcommands itself.
fn cli() -> clap::Command {
    use clap::CommandFactory;
    let cmd = Cli::command();
    let styles = annox_lsp::style::HELP;
    let (header, literal) = (styles.get_header(), styles.get_literal());
    let width = GROUPS.iter().flat_map(|(_, names)| names.iter().map(|n| n.len())).max().unwrap_or(0);
    let mut groups = String::new();
    for (heading, names) in GROUPS {
        groups.push_str(&format!("{header}{heading}:{header:#}\n"));
        for name in *names {
            let about = cmd.find_subcommand(name).and_then(|c| c.get_about()).map(|a| a.to_string());
            groups.push_str(&format!("  {literal}{name:width$}{literal:#}  {}\n", about.unwrap_or_default()));
        }
        groups.push('\n');
    }
    cmd.help_template(format!(
        "{{before-help}}{{about-with-newline}}\n{{usage-heading}} {{usage}}\n\n{groups}\
         {header}Options:{header:#}\n{{options}}{{after-help}}"
    ))
}

fn main() -> anyhow::Result<()> {
    use clap::FromArgMatches;
    let mut cmd = cli();
    let args = Cli::from_arg_matches(&cmd.get_matches_mut()).unwrap_or_else(|e| e.exit());
    let Some(command) = args.command else {
        // Like clap's `arg_required_else_help`, but with the title the long help has.
        anstream::eprint!("{}", cmd.render_long_help().ansi());
        std::process::exit(2);
    };
    match command {
        Command::Lsp => {
            let (connection, io_threads) = Connection::stdio();
            annox_lsp::run(&connection)?;
            drop(connection);
            io_threads.join()?;
            Ok(())
        }
        Command::Hub { data, listen, token_file, verbose } => {
            // A file, not a flag value, so the token doesn't show up in `ps`.
            let token = token_file.map(std::fs::read_to_string).transpose()?.map(|t| t.trim().to_owned());
            let listener = TcpListener::bind(&listen)?;
            eprintln!("annox hub listening on ws://{} (workspaces at /w/<name>)", listener.local_addr()?);
            serve(listener, HubConfig { data, token, verbose })?;
            Ok(())
        }
        Command::Mcp { root, author, name, channel, mention, settle } => {
            let root = root.map_or_else(std::env::current_dir, Ok)?;
            let channel =
                channel.then(|| annox_lsp::cli::Watch { file: None, reader: author.clone(), mention, settle });
            let config = annox_lsp::mcp::Config { root, author, name, channel };
            annox_lsp::mcp::serve(std::io::stdin().lock(), std::io::stdout(), &config)?;
            Ok(())
        }
        Command::Watch(options) => {
            let json = output_json(args.json);
            let stop = std::sync::atomic::AtomicBool::new(false);
            let print = |thread: serde_json::Value| match json {
                true => print_line(&thread.to_string()),
                false => print_line(&annox_lsp::render::threads(&serde_json::json!([thread]))),
            };
            let watched = annox_lsp::cli::Watcher::new(options, &std::env::current_dir()?).and_then(|watcher| {
                // Easy to miss: watching as yourself, your own comments never show up.
                eprintln!("annox: watching as {}; changes made as that id don't show up", watcher.reader());
                annox_lsp::cli::watch(watcher, &stop, print)
            });
            if let Err(e) = watched {
                fail(e);
            }
            Ok(())
        }
        Command::Annotations(annox_lsp::cli::Command::Log { file, follow: true }) => {
            let json = output_json(args.json);
            let stop = std::sync::atomic::AtomicBool::new(false);
            let print = |event: serde_json::Value| match json {
                true => print_line(&event.to_string()),
                false => print_line(&annox_lsp::render::log(&serde_json::json!([event]))),
            };
            if let Err(e) = annox_lsp::cli::follow_log(file.as_deref(), &std::env::current_dir()?, &stop, print) {
                fail(e);
            }
            Ok(())
        }
        Command::Annotations(command) => {
            let render = (!output_json(args.json)).then(|| annox_lsp::render::of(&command));
            match (annox_lsp::cli::execute(command, &std::env::current_dir()?), render) {
                (Ok(output), Some(render)) => {
                    anstream::print!("{}", render(&output));
                    Ok(())
                }
                (Ok(output), None) => {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                    Ok(())
                }
                (Err(e), _) => fail(e),
            }
        }
        Command::Update { version } => update(version),
        Command::Man { dir } => {
            let cmd = cli();
            match dir {
                Some(dir) => {
                    std::fs::create_dir_all(&dir)?;
                    clap_mangen::generate_to(cmd, &dir)?;
                }
                None => clap_mangen::Man::new(cmd).render(&mut std::io::stdout())?,
            }
            Ok(())
        }
    }
}

/// Whether to print JSON: with --json (`flag`), or as ANNOX_OUTPUT or the
/// config file say. Exits if ANNOX_OUTPUT is neither json nor text.
fn output_json(flag: bool) -> bool {
    annox_lsp::render::json_output(flag).unwrap_or_else(|e| fail(e))
}

/// Prints `text`, which may hold styles, ending it with a line break if it
/// doesn't, and flushes it, for output that streams.
fn print_line(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut out = anstream::stdout().lock();
    write!(out, "{text}")?;
    if !text.ends_with('\n') {
        writeln!(out)?;
    }
    Ok(out.flush()?)
}

/// Prints `e` as annox's errors read, and exits with status 1.
fn fail(e: anyhow::Error) -> ! {
    anstream::eprintln!("{} {e:#}", annox_lsp::style::paint(annox_lsp::style::ERROR, "annox:"));
    std::process::exit(1);
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
        super::cli().debug_assert();
    }

    #[test]
    fn help_groups_every_subcommand() {
        let grouped: Vec<&str> = super::GROUPS.iter().flat_map(|(_, names)| names.iter().copied()).collect();
        let cmd = super::cli();
        let shown: Vec<&str> = cmd.get_subcommands().filter(|c| !c.is_hide_set()).map(|c| c.get_name()).collect();
        let mut sorted = grouped.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), grouped.len(), "a subcommand is in two groups");
        for name in &shown {
            assert!(grouped.contains(name), "`annox {name}` is in no group of `annox --help`");
        }
        for name in &grouped {
            assert!(shown.contains(name), "`{name}` in GROUPS isn't a subcommand");
        }
    }
}
