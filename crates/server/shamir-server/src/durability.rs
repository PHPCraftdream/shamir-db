//! Server-specific restore facade, retaining the historical durability path.

pub use crate::boot::restore;
pub use crate::boot::restore::*;
pub use shamir_sync::durability::{backup, maintenance_lock, tx_registry, DurabilityConfig};
