//! Per-alias receipts of implicit (non-transactional) batches and the
//! typed `changes_since` read (VIS-5b). `mod.rs` is a manifest: types and
//! logic live in sibling files.

mod alias_receipt;
mod client_ops;
mod extract;
mod reconcile;

pub use alias_receipt::AliasReceipt;
pub(crate) use client_ops::{changes_since_via, decode_changes_since, execute_with_receipts_via};
pub use extract::extract_receipts;
pub use reconcile::{reconcile_receipts, RepoObservation};
