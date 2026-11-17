//! Command-line arguments for the offline `logical-import` subcommand.

/// Import a verified logical dump into a NEW server data directory.
/// The target must be absent or empty; nothing is merged or replaced.
/// Cancellation is via the process signal token, not a flag.
#[derive(Clone, Debug, clap::Args)]
pub struct LogicalImportArgs {
    /// Path to the logical-dump artifact to import.
    #[arg(long)]
    pub input: std::path::PathBuf,
    /// Server data directory to create; must be absent or empty.
    #[arg(long)]
    pub target: std::path::PathBuf,
    /// Require (and only accept) a schema-only artifact, mirroring
    /// `logical-export --schema-only`.
    #[arg(long)]
    pub schema_only: bool,
    /// Rename the imported database in every reference: the DatabaseItem
    /// name, records' repository scope, ACL resource refs and the
    /// catalogue ownership rows.
    #[arg(long)]
    pub as_db: Option<String>,
}
