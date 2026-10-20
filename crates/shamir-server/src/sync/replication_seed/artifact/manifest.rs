use serde::{Deserialize, Serialize};

use crate::replication_seed::{FileRecord, TableRecord};

/// SEED-2 v1: named MessagePack fields, ordered tables/files, no timestamps.
/// Digest is SHA-256 of `to_vec_named(self)`; Complete contains that digest.
/// This is integrity relative to a trusted digest, not authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format_version: u32,
    pub codec_version: u32,
    pub requirements: Vec<String>,
    pub source_db: String,
    pub source_repo: String,
    pub lineage_id: [u8; 16],
    pub history_epoch: u64,
    pub version: u64,
    pub certified_gap: Option<(u64, u64)>,
    pub schema_sha256: String,
    pub artifact_id: [u8; 16],
    pub index_policy: String,
    pub tables: Vec<TableRecord>,
    pub files: Vec<FileRecord>,
}
