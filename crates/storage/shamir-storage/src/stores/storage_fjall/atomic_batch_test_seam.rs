//! Test-only failure-injection seam for the repository-wide atomic batch commit.
use std::sync::atomic::{AtomicBool, Ordering};

pub static FAIL_BEFORE_COMMIT: AtomicBool = AtomicBool::new(false);
pub static FAIL_AFTER_COMMIT: AtomicBool = AtomicBool::new(false);

pub fn reset() {
    FAIL_BEFORE_COMMIT.store(false, Ordering::Release);
    FAIL_AFTER_COMMIT.store(false, Ordering::Release);
}
