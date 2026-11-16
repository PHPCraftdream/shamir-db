use serde::{Deserialize, Serialize};

/// Data-directory journal file for resumable format migrations.
pub const JOURNAL_FILE_NAME: &str = "format_migration.journal";

/// One durable state transition in the format-migration journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalRecord {
    ChainStarted {
        from_epoch: u64,
        to_epoch: u64,
        binary_version: String,
        modules: Vec<String>,
        unix_ms: u64,
    },
    StepStarted {
        module: String,
        from_epoch: u64,
        to_epoch: u64,
        unix_ms: u64,
    },
    StepFinished {
        module: String,
        unix_ms: u64,
    },
    ChainFinished {
        to_epoch: u64,
        unix_ms: u64,
    },
}
