use serde::{Deserialize, Serialize};

/// Additive status DTO. Source and destination positions are separate domains.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeedRepositoryStatus {
    pub installation_state: Option<String>,
    pub artifact_digest: Option<String>,
    pub source_db: Option<String>,
    pub source_repo: Option<String>,
    pub source_lineage_id: Option<[u8; 16]>,
    pub source_history_epoch: Option<u64>,
    pub seed_source_version: Option<u64>,
    pub upstream_bookmark: Option<u64>,
    pub next_pull_version: Option<u64>,
    pub local_baseline_version: Option<u64>,
    pub local_consistent_version: Option<u64>,
    pub schema_digest: Option<String>,
    pub certified_gap_first: Option<u64>,
    pub certified_gap_through: Option<u64>,
    pub subscription_state: Option<String>,
    pub last_terminal_reason: Option<String>,
}
