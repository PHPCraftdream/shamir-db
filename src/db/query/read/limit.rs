//! LimitOffset — LIMIT and OFFSET clause.

use serde::{Deserialize, Serialize};

/// LIMIT and OFFSET
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitOffset {
    /// Maximum records to return
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Records to skip
    #[serde(default)]
    pub offset: u64,
}

impl LimitOffset {
    pub fn new(limit: impl Into<Option<u64>>) -> Self {
        LimitOffset {
            limit: limit.into(),
            offset: 0,
        }
    }

    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = offset;
        self
    }

    pub fn no_limit() -> Self {
        LimitOffset {
            limit: None,
            offset: 0,
        }
    }
}

impl Default for LimitOffset {
    fn default() -> Self {
        Self::no_limit()
    }
}
