//! The annotation commands' arguments.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// The annotation commands. `annox` flattens these into its own subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Print what everyone did to the annotations of FILE, or of every document, oldest first
    Log {
        /// A document in the workspace
        file: Option<String>,
        /// Keep running, and print new events as they're written, instead of the ones so far
        #[arg(short, long)]
        follow: bool,
    },
    /// Create a workspace in DIR (default: the current directory)
    Init {
        /// The project root
        dir: Option<PathBuf>,
        /// Keep `.annox/` out of version control, so annotations aren't shared
        #[arg(long)]
        local: bool,
    },
    /// Print the threads on FILE, or on every document
    List {
        /// A document in the workspace
        file: Option<String>,
        /// Also list resolved, accepted, rejected, withdrawn and deleted annotations
        #[arg(long)]
        all: bool,
        /// Also list resolved, accepted, rejected and withdrawn annotations, but not deleted ones
        #[arg(long, conflicts_with = "all")]
        closed: bool,
        /// Whose read state `unread` uses, and who `--mine` and `--others` mean [default: the identity write
        /// commands use]
        #[arg(long = "as", value_name = "ID")]
        reader: Option<String>,
        #[command(flatten)]
        filter: Filter,
    },
    /// Summarize the annotations of FILE, or of every document, one line per document or author
    Report {
        /// A document in the workspace
        file: Option<String>,
        /// Break each line's counts down by status, one line per kind
        #[arg(short, long)]
        verbose: bool,
        /// Group by document or by author; comma-separated to nest, as in `author,document`. An author counts
        /// the suggestions, comments and replies they wrote [default: document]
        #[arg(long, value_name = "GROUP", value_delimiter = ',', value_parser = ["document", "author"])]
        by: Vec<String>,
        /// Whose read state `unread` uses, and who `--mine` and `--others` mean [default: the identity write
        /// commands use]
        #[arg(long = "as", value_name = "ID")]
        reader: Option<String>,
        #[command(flatten)]
        filter: Filter,
    },
    /// Print one annotation and its thread
    Show {
        /// The annotation's id, as printed by `annox list`; a reply's shows its thread
        id: String,
    },
    /// Print who did what to a thread (the annotation and its replies), and when, oldest first
    History {
        /// The annotation's id, as printed by `annox list`; a reply's shows its thread
        id: String,
    },
    /// Comment on a quote in FILE
    Comment {
        /// The document to annotate
        file: String,
        #[command(flatten)]
        at: Quote,
        /// The comment
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: String,
        #[command(flatten)]
        meta: Meta,
        #[command(flatten)]
        author: Author,
    },
    /// Highlight a quote in FILE: a comment with no body
    Highlight {
        /// The document to annotate
        file: String,
        #[command(flatten)]
        at: Quote,
        #[command(flatten)]
        meta: Meta,
        #[command(flatten)]
        author: Author,
    },
    /// Suggest a replacement for a quote in FILE
    Suggest {
        /// The document to annotate
        file: String,
        #[command(flatten)]
        at: Quote,
        /// The text to put in place of the quote
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        replace: String,
        /// An explanation of the suggestion
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: Option<String>,
        #[command(flatten)]
        meta: Meta,
        #[command(flatten)]
        author: Author,
    },
    /// Reply to a comment or suggestion
    Reply {
        /// The annotation's id, as printed by `annox list`
        id: String,
        /// The reply
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: String,
        #[command(flatten)]
        author: Author,
    },
    /// Change the body or label of an annotation
    Edit {
        /// The annotation's id, as printed by `annox list`
        id: String,
        /// The new body
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: Option<String>,
        /// The new label; an empty one removes it
        #[arg(long, value_name = "L", allow_hyphen_values = true)]
        label: Option<String>,
        #[command(flatten)]
        author: Author,
    },
    /// Set a status: open or resolved (comments), open, rejected or withdrawn (suggestions)
    Status {
        /// The annotation's id, as printed by `annox list`
        id: String,
        /// The new status
        status: String,
        #[command(flatten)]
        author: Author,
    },
    /// Apply a suggestion to its file and mark it accepted
    Accept {
        /// The annotation's id, as printed by `annox list`
        id: String,
        /// Accept a suggestion that was relocated, after checking where it now applies
        #[arg(long)]
        confirmed: bool,
        #[command(flatten)]
        author: Author,
    },
    /// Undo an accepted suggestion with a new suggestion that restores the original text, or the open one
    /// that already does
    Revert {
        /// The accepted suggestion's id, as printed by `annox list --closed`
        id: String,
        /// Accept the new suggestion right away, instead of leaving it open for review
        #[arg(long)]
        accept: bool,
        /// Why it's being reverted, in Markdown
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: Option<String>,
        #[command(flatten)]
        author: Author,
    },
    /// Move an open suggestion to new text, with a reviewed replacement
    Retarget {
        /// The annotation's id, as printed by `annox list`
        id: String,
        #[command(flatten)]
        at: MoveTo,
        /// The replacement, reviewed against the new text
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        replace: String,
        #[command(flatten)]
        author: Author,
    },
    /// Move an open comment to new text
    Reattach {
        /// The annotation's id, as printed by `annox list`
        id: String,
        #[command(flatten)]
        at: MoveTo,
        #[command(flatten)]
        author: Author,
    },
    /// Hide an annotation (its events are kept)
    Delete {
        /// The annotation's id, as printed by `annox list`
        id: String,
        #[command(flatten)]
        author: Author,
    },
    /// Undo a delete
    Restore {
        /// The annotation's id, as printed by `annox list`
        id: String,
        #[command(flatten)]
        author: Author,
    },
    /// React to an annotation or reply with an emoji; 👍 on a suggestion approves it
    React {
        /// The annotation's id, as printed by `annox list`
        id: String,
        /// The emoji, such as 👍 or 👀
        reaction: String,
        /// Remove the reaction instead
        #[arg(long)]
        remove: bool,
        #[command(flatten)]
        author: Author,
    },
    /// Mark threads read, so `annox list --unread` stops listing them until someone changes them again
    Read {
        /// Annotation ids, as printed by `annox list`; a reply's marks its thread
        #[arg(required_unless_present = "all")]
        ids: Vec<String>,
        /// Mark every thread read
        #[arg(long, conflicts_with = "ids")]
        all: bool,
        #[command(flatten)]
        author: Author,
    },
    /// Commit the uncommitted annotation events to git, and nothing else
    Commit {
        /// The commit message [default: a summary of what's being committed]
        #[arg(long, short, value_name = "TEXT", allow_hyphen_values = true)]
        message: Option<String>,
        /// Print what would be committed, without committing
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the settings in effect here, such as who write commands write as, and where each comes from
    ///
    /// The settings are read from ~/.config/annox/config.json, ~/.config/annox/credentials.json,
    /// the workspace's .annox/annox.json, and the environment. A file that's missing or invalid is
    /// listed as such.
    Config,
}

/// Options of `annox watch`, and of the channel of `annox mcp`.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct Watch {
    /// A document in the workspace [default: every document]
    pub file: Option<String>,
    /// Whose read state to follow; their own changes don't count [default: the identity write commands use]
    #[arg(long = "as", value_name = "ID")]
    pub reader: Option<String>,
    /// Only threads containing TEXT (case-insensitive), such as "@claude", or that the reader wrote in
    /// (repeatable)
    #[arg(long, value_name = "TEXT")]
    pub mention: Vec<String>,
    /// How long a thread must stay unchanged before it's printed, in milliseconds, so a reply still being
    /// written isn't picked up half done
    #[arg(long, value_name = "MS", default_value_t = 2000)]
    pub settle: u64,
}

/// The text a command targets.
#[derive(clap::Args, Debug)]
pub struct Quote {
    /// The text to target, exactly as it appears in the file
    #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
    pub(super) quote: String,
    /// Which occurrence of the quote to target (1-based), when it appears more than once
    #[arg(long, value_name = "N")]
    pub(super) occurrence: Option<usize>,
}

/// Where `retarget` and `reattach` move an annotation.
#[derive(clap::Args, Debug)]
pub struct MoveTo {
    /// The text to target, exactly as it appears in the file
    #[arg(long, value_name = "TEXT", allow_hyphen_values = true, required_unless_present = "suggested")]
    pub(super) quote: Option<String>,
    /// Which occurrence of the quote to target (1-based), when it appears more than once
    #[arg(long, value_name = "N", requires = "quote")]
    pub(super) occurrence: Option<usize>,
    /// Target the suggested location `annox list` shows for an orphaned annotation
    #[arg(long, conflicts_with = "quote")]
    pub(super) suggested: bool,
}

/// Which annotations `list` prints and `report` counts. Filters combine with AND.
#[derive(clap::Args, Debug, Default)]
#[command(next_help_heading = "Filters")]
pub struct Filter {
    /// Only comments or only suggestions
    #[arg(long, value_name = "KIND", value_parser = ["comment", "suggestion"])]
    pub(super) kind: Option<String>,
    /// Only these statuses (repeatable or comma-separated) [default: open, or any with --all, --closed or --quote;
    /// any in `report`]
    #[arg(long, value_name = "S", value_delimiter = ',',
          value_parser = ["open", "resolved", "accepted", "rejected", "withdrawn"])]
    pub(super) status: Vec<String>,
    /// Only annotations by this author id (repeatable)
    #[arg(long, value_name = "ID")]
    pub(super) author: Vec<String>,
    /// Skip annotations by this author id (repeatable)
    #[arg(long, value_name = "ID")]
    pub(super) not_author: Vec<String>,
    /// Only your own annotations (the identity write commands use)
    #[arg(long, conflicts_with = "others")]
    pub(super) mine: bool,
    /// Skip your own annotations (the identity write commands use)
    #[arg(long)]
    pub(super) others: bool,
    /// Only open annotations that need fixing: orphaned, or suggestions that can't be applied
    #[arg(long)]
    pub(super) broken: bool,
    /// Only threads others changed since you last read them (see `annox read`), closed ones included
    #[arg(long)]
    pub(super) unread: bool,
    /// Only annotations on this text in FILE, closed ones included. An accepted suggestion is on the text it put in
    #[arg(long, value_name = "TEXT", allow_hyphen_values = true, requires = "file")]
    pub(super) quote: Option<String>,
    /// Which occurrence of the quote to look at (1-based), when it appears more than once
    #[arg(long, value_name = "N", requires = "quote")]
    pub(super) occurrence: Option<usize>,
}

/// Options of a new comment or suggestion.
#[derive(clap::Args, Debug)]
pub struct Meta {
    /// A label, such as "question" or "typo"
    #[arg(long, value_name = "L", allow_hyphen_values = true)]
    pub(super) label: Option<String>,
    /// Keep it in the local area, which isn't shared
    #[arg(long)]
    pub(super) local: bool,
}

/// Who a write command writes as.
#[derive(clap::Args, Debug)]
#[command(next_help_heading = "Author")]
pub struct Author {
    /// Author id [default: ANNOX_AUTHOR, then ~/.config/annox/config.json, then git's identity]
    #[arg(long, value_name = "ID")]
    pub(super) author: Option<String>,
    /// Display name [default: ANNOX_AUTHOR_NAME, then the config file, then git]
    #[arg(long, value_name = "NAME", allow_hyphen_values = true)]
    pub(super) name: Option<String>,
}

#[derive(Parser)]
#[command(name = "annox")]
pub(super) struct Cli {
    #[command(subcommand)]
    pub(super) command: Command,
}
