use std::path::PathBuf;

use clap::Args;

/// Offline format-migration controls for a stopped server data directory.
#[derive(Debug, Clone, Args)]
pub struct FormatMigrateArgs {
    /// Print a read-only format-migration plan and exit.
    #[arg(long)]
    pub plan: bool,
    /// Snapshot root produced by `backup --to` (the directory containing `manifest.json`).
    #[arg(long, value_name = "DIR")]
    pub from_backup: Option<PathBuf>,
    /// Maximum accepted backup age in hours (default: 24).
    #[arg(long, value_name = "N", default_value_t = 24)]
    pub max_backup_age_hours: u64,
    /// Explicitly bypass backup verification; emits a warning on stderr.
    #[arg(long)]
    pub i_have_a_backup: bool,
    /// Required to execute the format migration; without it, print a plan.
    #[arg(long)]
    pub yes: bool,
}
