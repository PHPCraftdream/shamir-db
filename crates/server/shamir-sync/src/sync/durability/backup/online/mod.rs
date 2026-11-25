//! Online backup copy engine (design `online-backup.md` 4.1, 4.2).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use crate::sync::durability::backup::{utc_timestamp, utc_timestamp_at};

#[path = "copy/copy_engine.rs"]
pub mod copy_engine;
#[path = "copy/copy_types.rs"]
mod copy_types;
pub mod manifest;
pub mod progress;
pub mod protocol;
pub mod wal_capture;

pub use protocol::CleanupFailure;

#[cfg(test)]
mod tests;

/// TEST SEAM: the base epoch second of the snapshot-directory name clock.
///
/// Unset (or `0`) means the REAL clock: [`snapshot_name`] then returns
/// exactly what `utc_timestamp()` returns today — the default path is
/// byte-identical. A test that drives more than one online backup inside one
/// wall-clock second installs a base with
/// [`set_snapshot_name_base_seconds`]; every following name is then
/// `base + run` for the process-wide monotonic run counter below, so two
/// runs never share a second (they would otherwise collide on the atomic
/// `<to>/<timestamp>` create).
///
/// First-install-wins, like the engine's step-hook slot, and deliberately
/// not `#[cfg(test)]`: the `shamir-server` fixtures install it, and their
/// tests do not compile this crate's `#[cfg(test)]` code. Production never
/// calls the setter, so the read path never observes it.
static SNAPSHOT_NAME_BASE_SECONDS: OnceLock<AtomicU64> = OnceLock::new();

/// Monotonic counter of INJECTED names (see
/// [`SNAPSHOT_NAME_BASE_SECONDS`]). Only advances while the clock is
/// injected, so the default path never touches it.
static SNAPSHOT_NAME_RUN: AtomicU64 = AtomicU64::new(0);

/// Install the TEST clock: snapshot names become `base + run` instead of
/// "now" (see [`SNAPSHOT_NAME_BASE_SECONDS`]). `0`, and never calling this,
/// keep the real clock. First call wins; later calls are ignored.
pub fn set_snapshot_name_base_seconds(base: u64) {
    let _ = SNAPSHOT_NAME_BASE_SECONDS.set(AtomicU64::new(base));
}

/// The injected name second for THIS run, or `None` for the real clock.
fn injected_name_seconds() -> Option<u64> {
    let base = SNAPSHOT_NAME_BASE_SECONDS.get()?.load(Ordering::Relaxed);
    if base == 0 {
        return None;
    }
    Some(base + SNAPSHOT_NAME_RUN.fetch_add(1, Ordering::Relaxed))
}

/// The name of one run's snapshot directory: `YYYYMMDD_HHMMSS` UTC.
///
/// The real clock by default; the injected, per-run UNIQUE second when a
/// test installed the name clock ([`set_snapshot_name_base_seconds`]).
pub fn snapshot_name() -> String {
    match injected_name_seconds() {
        Some(seconds) => utc_timestamp_at(seconds),
        None => utc_timestamp(),
    }
}
