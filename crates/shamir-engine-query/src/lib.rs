#![deny(clippy::unwrap_used, clippy::expect_used)]

//! Query evaluation leaf split from shamir-engine (#1548 S1).
//! `query` preserves the moved code's `crate::query` paths.

#[cfg(test)]
use std::sync::atomic::Ordering;

use shamir_index as index2;
mod function;
mod index {
    pub use shamir_index::base_index::sorted_index_manager;
}
pub mod query;

#[cfg(test)]
fn assert_ticker_progress(ticker: &std::sync::atomic::AtomicU64) {
    let before = ticker.load(Ordering::Relaxed);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let after = ticker.load(Ordering::Relaxed);
    assert!(
        after.saturating_sub(before) >= 3,
        "ticker stalled: {before} -> {after}"
    );
}
