use serde::{Deserialize, Serialize};

use super::section::{SectionInventory, SectionKind};

/// Self-describing logical-dump header. Application schema is explicitly null
/// at the current (pre-R1c) compatibility boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalDumpHeader {
    pub format_version: u16,
    pub required_features: Vec<String>,
    pub source_storage_epoch: Option<u32>,
    pub application_schema_version: Option<u64>,
    pub snapshot_instant: u64,
    pub repository_versions: Vec<RepositoryVersion>,
    pub scope: DumpScope,
    pub encryption: EncryptionMode,
    pub sections: Vec<SectionInventory>,
}

impl LogicalDumpHeader {
    /// Validate version/feature/section declarations before any writer uses it.
    pub fn validate(&self, supported_features: &[&str]) -> Result<(), &'static str> {
        if self.format_version != 1 {
            return Err("unsupported logical-dump format version");
        }
        if self.application_schema_version.is_some() {
            return Err("application-schema versions are unsupported at the R1c boundary");
        }
        if self.encryption != EncryptionMode::None {
            return Err("unsupported logical-dump encryption mode");
        }
        if self
            .required_features
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err("required features must be unique and sorted");
        }
        if self.required_features.iter().any(|feature| {
            !supported_features
                .iter()
                .any(|supported| *supported == feature)
        }) {
            return Err("unknown required logical-dump feature");
        }
        if self.sections.len() != SectionKind::ORDERED.len() {
            return Err("logical-dump section inventory is incomplete");
        }
        if self
            .sections
            .iter()
            .zip(SectionKind::ORDERED)
            .any(|(entry, kind)| entry.section != kind)
        {
            return Err("logical-dump section inventory has invalid order");
        }
        if self.scope.table.is_some() && self.scope.repository.is_none() {
            return Err("table scope requires a repository");
        }
        if self
            .repository_versions
            .windows(2)
            .any(|pair| pair[0].repository >= pair[1].repository)
        {
            return Err("repository versions must be unique and sorted");
        }
        Ok(())
    }
}

/// Source repository version captured at the dump's logical instant.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryVersion {
    pub repository: String,
    pub version: u64,
}

/// Explicit selected scope; `table` is interpreted only within `repository`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DumpScope {
    pub repository: Option<String>,
    pub table: Option<String>,
    pub schema_only: bool,
    pub include_artifacts: bool,
}

/// Encryption is deliberately absent in the baseline format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncryptionMode {
    None,
}
