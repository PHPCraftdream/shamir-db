//! Index definition
//!
//! Defines a single index by path.

use serde::{Deserialize, Serialize};

/// Definition of a single index
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexInfoItem {
    /// Path to indexed field (interned components)
    pub path: Vec<u64>,
}

impl IndexInfoItem {
    /// Create an index definition
    pub fn new(path: Vec<u64>) -> Self {
        Self { path }
    }
}
