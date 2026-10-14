//! Per-repo durability override record codec: encode/decode the sparse
//! [`DurabilityOverride`] stored inside the system-store `repositories`
//! row (key `durability`), plus the ALTER-time merge helper.

mod record;

pub(crate) use record::{durability_from_record, durability_to_record, merge_override};
