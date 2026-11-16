use serde::{Deserialize, Serialize};

use super::section::SectionKind;

/// Explicit final trailer record, separate from the `End` section sentinel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trailer {
    pub preceding_frames: u64,
    pub preceding_items: u64,
    pub preceding_sections: u64,
    pub sections: Vec<TrailerSection>,
    pub preceding_bytes: u64,
    pub prefix_sha256: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrailerSection {
    pub section: SectionKind,
    pub items: u64,
    pub digest: [u8; 32],
}
