//! `rust-guard` — source-integrity gates for large structural refactors.
//!
//! Thin dispatcher only: parses the CLI, routes to the subcommand
//! implementation, and maps its result to a process exit code.
//! Findings print as `path:line: code: message` and exit non-zero.

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

mod conservation_cmd;
mod exceptions;
mod finding;
mod gitfs;
mod modules_cmd;
mod structure_cmd;

#[derive(Parser)]
#[command(
    name = "rust-guard",
    about = "Source-integrity gates (module reachability, structure, conservation)",
    version
)]
struct Cli {
    /// Repository root to operate on (default: the current directory).
    #[arg(long, global = true, default_value = ".")]
    repo: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Module reachability: every tracked `.rs` file under `crates/*/`
    /// and `tools/*/` must be reachable from a target root or excepted.
    Modules {
        /// Git revision to scan instead of the working tree (e.g. HEAD~1).
        #[arg(long)]
        rev: Option<String>,
        /// Path to the exceptions file (default: .rust-guard/exceptions.toml).
        #[arg(long)]
        exceptions: Option<String>,
    },
    /// AST rules for changed/moved areas (CI-03a chunk B).
    Structure {
        /// Glob(s) matched against git-tracked paths (repeatable).
        #[arg(long = "paths")]
        paths: Vec<String>,
        /// Select files changed since this revision (its merge base with
        /// HEAD becomes the implicit --base).
        #[arg(long)]
        changed_since: Option<String>,
        /// Base revision for the widening/allow rules.
        #[arg(long)]
        base: Option<String>,
        /// TOML delta file listing accepted widenings/allow additions.
        #[arg(long)]
        delta: Option<String>,
        /// Path to the exceptions file (default: .rust-guard/exceptions.toml).
        #[arg(long)]
        exceptions: Option<String>,
        /// Opt-in WARNING (not a finding) for files with more than one
        /// public export.
        #[arg(long, default_value_t = false)]
        warn_primary_export: bool,
    },
    /// Source inventory diff between two revisions (CI-03a chunk B).
    Conservation {
        /// Base revision (the "before" side). Must name a commit.
        #[arg(long)]
        base: String,
        /// Head revision (the "after" side).
        #[arg(long)]
        head: String,
        /// Comma-separated package filter (default: all packages).
        #[arg(long, value_delimiter = ',')]
        packages: Vec<String>,
        /// TOML delta file of declared allowances (optional; without it
        /// every difference is a finding).
        #[arg(long)]
        delta: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let repo = PathBuf::from(&cli.repo);
    match cli.command {
        Command::Modules { rev, exceptions } => {
            let code = modules_cmd::run(rev.as_deref(), exceptions.as_deref(), &repo);
            ExitCode::from(code)
        }
        Command::Structure {
            paths,
            changed_since,
            base,
            delta,
            exceptions,
            warn_primary_export,
        } => ExitCode::from(structure_cmd::run(structure_cmd::StructureArgs {
            repo,
            paths,
            changed_since,
            base,
            delta,
            exceptions,
            warn_primary_export,
        })),
        Command::Conservation {
            base,
            head,
            packages,
            delta,
        } => ExitCode::from(conservation_cmd::run(conservation_cmd::ConservationArgs {
            repo,
            base,
            head,
            packages,
            delta,
        })),
    }
}
