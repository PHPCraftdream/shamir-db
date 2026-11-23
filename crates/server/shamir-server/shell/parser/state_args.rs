//! The narrow-data and two-word `state`-family command grammar tails
//! (design `docs/dev-artifacts/design/admin-shell.md` §§4.2, §4.4): the
//! clap subcommand enums projected by [`super::grammar`] onto the state
//! worker's plain [`crate::shell::commands::StateCommand`]. Grammar only:
//! no I/O, no session state, and no key decoding — Base58 validation of a
//! `data get` KEY happens after the §3.3 token gate, in the worker's
//! `validate`, before any connect or write.

#[derive(clap::Subcommand)]
pub(super) enum VersionCommand {
    /// Show the observed format epoch and stamp outcome (no disk read).
    Show,
    /// Show the boot-captured format-stamp state (the P1 status view).
    Stamp,
    /// Explained only: the manual offline format migration.
    Migrate,
}

#[derive(clap::Subcommand)]
pub(super) enum ReplicationCommand {
    /// List replication subscriptions (requires the profile database).
    Subscriptions,
}

/// The explicit-database narrow views; `head` promises no ordering.
#[derive(clap::Subcommand)]
pub(super) enum DataCommand {
    /// Count the rows of one table.
    Count {
        database: String,
        repo: String,
        table: String,
    },
    /// First N rows in server scan order (no ordering promise).
    Head {
        database: String,
        repo: String,
        table: String,
        rows: u64,
    },
    /// Exact-key point read; KEY is Base58 (validated in the state worker,
    /// after the §3.3 token gate and before any connect).
    Get {
        database: String,
        repo: String,
        table: String,
        key: String,
    },
}
