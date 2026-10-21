//! Online/unique index admission impl blocks (`impl TableManager`).
//!
//! NOTE (engine sweep T1, chunk W3): NOT yet declared by any parent manifest.
//! See `../barrier/mod.rs` for the mounting rationale (chunk W4 wires this).

mod write_domain_index_admitted;
mod write_domain_index_core;
mod write_domain_online_index;
