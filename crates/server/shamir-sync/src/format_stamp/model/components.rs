//! Initial per-component format versions at the base epoch (design 4.1).

use serde_json::{Map, Value};

/// Component names and versions stamped at the base epoch (all `1`).
const BASE_COMPONENT_VERSIONS: [(&str, u32); 7] = [
    ("wal", 1),
    ("storage", 1),
    ("system_catalogue", 1),
    ("index_meta", 1),
    ("vector_snapshot", 1),
    ("interner", 1),
    ("audit", 1),
];

/// Fresh `components` object for a new stamp.
pub fn initial_components() -> Map<String, Value> {
    BASE_COMPONENT_VERSIONS
        .iter()
        .map(|(name, version)| ((*name).to_string(), Value::from(*version)))
        .collect()
}
