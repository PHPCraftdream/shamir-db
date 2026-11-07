//! Cross-cutting batch controls: the authorization gate, the op watchdog
//! and write-receipt commit visibility propagation.

pub(super) mod authorized;
pub(super) mod op_watchdog;
pub(crate) mod receipts;
