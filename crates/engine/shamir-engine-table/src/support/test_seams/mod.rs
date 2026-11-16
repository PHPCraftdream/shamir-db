//! Test-seam manifest: test-only hooks and overlays.

#[cfg(any(test, feature = "test-util"))]
pub mod index2_backfill_hook;
#[cfg(any(test, feature = "test-util"))]
pub mod test_support;
#[doc(hidden)]
pub mod tx_scan_overlay;
