//! Terminal colors for the CLI's text output and help.
//!
//! The renderers always emit ANSI escapes; print them through `anstream`,
//! which strips them when the output isn't a terminal or `NO_COLOR` is set.

use anstyle::{AnsiColor, Style};

/// The highlighter yellow of annox's logo.
pub const ACCENT: Style = AnsiColor::Yellow.on_default().bold();
pub const PATH: Style = Style::new().bold();
pub const AUTHOR: Style = AnsiColor::Cyan.on_default();
pub const MUTED: Style = Style::new().dimmed();
pub const OPEN: Style = AnsiColor::Yellow.on_default();
pub const DONE: Style = AnsiColor::Green.on_default();
pub const PROBLEM: Style = AnsiColor::Red.on_default();
pub const PENDING: Style = AnsiColor::Cyan.on_default();
pub const ERROR: Style = AnsiColor::Red.on_default().bold();

/// `text` in `style`.
pub fn paint(style: Style, text: impl std::fmt::Display) -> String {
    format!("{style}{text}{style:#}")
}

/// The style of a status or count name in `report`, or of an event's verb
/// in `log` and `history`.
pub fn of(name: &str) -> Style {
    match name {
        "open" | "commented" | "suggested" | "replied" | "reopened" => OPEN,
        "resolved" | "accepted" | "restored" => DONE,
        "rejected" | "deleted" | "orphaned" | "stale" | "conflicted" | "file missing" => PROBLEM,
        "withdrawn" | "withdrew" | "closed" => MUTED,
        "uncommitted" | "changes" | "change" => PENDING,
        _ => Style::new(),
    }
}

/// clap's help and errors, in the same palette.
pub const HELP: clap::builder::Styles = clap::builder::Styles::styled()
    .header(ACCENT.underline())
    .usage(ACCENT.underline())
    .literal(Style::new().bold())
    .placeholder(AUTHOR)
    .valid(DONE)
    .invalid(OPEN)
    .error(ERROR);

/// The title atop `annox --help`.
pub fn banner() -> String {
    #[rustfmt::skip]
    let art = [
        " ▄▄▄  ▄▄  ▄▄ ▄▄  ▄▄  ▄▄▄  ▄▄ ▄▄",
        "██▀██ ███▄██ ███▄██ ██▀██ ▀█▄█▀",
        "██▀██ ██ ▀██ ██ ▀██ ▀███▀ ██ ██",
    ];
    art.map(|line| paint(ACCENT, line)).join("\n")
}
