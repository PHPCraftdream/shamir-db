//! Field-bound validation error DTO.

use crate::filter::FieldPath;
use serde::{Deserialize, Serialize};

/// A single field-bound validation error (codes only -- no human text).
///
/// `field = None` means a record-level error (e.g. "at least one
/// contact method required"). `code` is a stable, machine-readable
/// key for i18n / programmatic handling on the client side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    /// Path into the record's field tree, or `None` for record-level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<FieldPath>,
    /// Stable machine-readable error code.
    pub code: String,
}
