//! Database-level DDL operations: create / drop database.

use serde::{Deserialize, Serialize};

/// Serde skip-serializing-if helper: omit `false` booleans from the wire.
pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

/// Create a new database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateDbOp {
    pub create_db: String,
    /// When `true`, a pre-existing database with the same name is NOT an
    /// error — the operation returns `{"created": false, "existed": true}`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_not_exists: bool,
}

/// Drop a database.
///
/// Requires an `hmac` field — hex-encoded HMAC-SHA256 tag over
/// `b"drop_db\0<db_name>"` keyed by the session HMAC key
/// (`SHA256("shamir-db hmac key v1\0" || session_id)`). Missing /
/// wrong tag → request rejected with `hmac_required` /
/// `hmac_mismatch`. See `query_buffer_config.rs` design notes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropDbOp {
    pub drop_db: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
    /// When `true`, all repositories (and their tables) inside the database
    /// are removed recursively before the database itself is dropped.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cascade: bool,
    /// When `true`, dropping a non-existent database is a silent no-op
    /// returning `{"existed": false}` instead of an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_exists: bool,
}
