//! The decoded stamp body (`FormatStamp`) and its constructors (design 2.3).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

use super::components::initial_components;
use super::epochs::CURRENT_FORMAT_EPOCH;

/// Actor recorded in `created_by` / `last_opened_by` (design 2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StampActor {
    pub version: String,
    pub unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted: Option<bool>,
}

/// `last_format_migration`; `null` until R1b writes a real entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastFormatMigration {
    pub id: String,
    pub from_epoch: u64,
    pub to_epoch: u64,
    pub unix_ms: u64,
}

/// The decoded stamp. Private fields; access through the getters. The
/// `#[serde(flatten)] extra` map preserves unknown body keys across a
/// read-modify-write (decision D3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormatStamp {
    format_epoch: u64,
    components: Map<String, Value>,
    created_by: StampActor,
    #[serde(default)]
    last_opened_by: Option<StampActor>,
    #[serde(default)]
    last_format_migration: Option<LastFormatMigration>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl FormatStamp {
    /// Stamp for a directory this binary creates, at `format_epoch`.
    pub fn with_epoch(format_epoch: u64, version: impl Into<String>, unix_ms: u64) -> Self {
        let actor = StampActor {
            version: version.into(),
            unix_ms,
            adopted: None,
        };
        Self {
            format_epoch,
            components: initial_components(),
            created_by: actor.clone(),
            last_opened_by: Some(actor),
            last_format_migration: None,
            extra: Map::new(),
        }
    }

    /// Stamp for a directory this binary creates at the current epoch.
    pub fn new(version: impl Into<String>, unix_ms: u64) -> Self {
        Self::with_epoch(CURRENT_FORMAT_EPOCH, version, unix_ms)
    }

    /// Row-5a adoption of stampless data: `created_by.adopted = true`.
    pub fn adopted(format_epoch: u64, version: impl Into<String>, unix_ms: u64) -> Self {
        let mut stamp = Self::with_epoch(format_epoch, version, unix_ms);
        stamp.created_by.adopted = Some(true);
        stamp
    }

    /// D6: replace `last_opened_by` (called only when the version changed).
    pub fn with_last_opened(mut self, version: impl Into<String>, unix_ms: u64) -> Self {
        self.last_opened_by = Some(StampActor {
            version: version.into(),
            unix_ms,
            adopted: None,
        });
        self
    }

    /// Design 4.5: set by the format-migration runner, stamp written last.
    pub fn with_last_format_migration(mut self, migration: LastFormatMigration) -> Self {
        self.last_format_migration = Some(migration);
        self
    }

    pub fn format_epoch(&self) -> u64 {
        self.format_epoch
    }

    pub fn components(&self) -> &Map<String, Value> {
        &self.components
    }

    pub fn created_by(&self) -> &StampActor {
        &self.created_by
    }

    pub fn last_opened_by(&self) -> Option<&StampActor> {
        self.last_opened_by.as_ref()
    }

    pub fn last_format_migration(&self) -> Option<&LastFormatMigration> {
        self.last_format_migration.as_ref()
    }

    /// Unknown body keys preserved from a foreign writer (design 2.3).
    pub fn extra(&self) -> &Map<String, Value> {
        &self.extra
    }
}

/// Current wall-clock time in whole milliseconds since the Unix epoch.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
