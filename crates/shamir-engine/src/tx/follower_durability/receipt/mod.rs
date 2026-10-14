//! Receipt-based follower apply/resume protocol (S5c-X PR6b-2).
//!
//! Algorithm 1-5 live in the sibling modules: `stage` builds the receipt and
//! performs the Prepared / Materialized atomic writes, `checkpoint` validates
//! journal coverage and retires the receipt atomically with the bookmark
//! advance, `resume` re-owns an outstanding receipt after a crash, `publish`
//! raises visibility post-durability, and `apply` orchestrates the whole
//! batch. `activation` decides when this path replaces the legacy apply.

mod activation;
mod apply;
// Cold-start recovery (cold_reconcile) reaches the pipeline internals
// through these modules.
pub(crate) mod checkpoint;
mod publish;
mod resume;
pub(crate) mod stage;

pub(crate) use activation::receipt_path_selected;
pub(crate) use apply::apply_batch_receipt;

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
