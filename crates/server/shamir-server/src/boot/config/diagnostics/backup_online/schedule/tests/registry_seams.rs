//! Test-only arming seams of `OnlineBackupRegistry`, moved out of
//! `registry.rs` for layout (the impl body is unchanged). This module is a
//! CHILD of `registry` (declared by `#[path]` from `registry.rs`), not of
//! `schedule::tests`: the seam impl touches the registry's private fields, so
//! it lives next to them, and every call path is unchanged.
//!
//! The engine-step dispatcher itself lives in its OWN `schedule` module
//! (`schedule::online_backup_step_hooks`, declared by `#[path]` over
//! `schedule/tests/online_backup_step_hooks.rs`) and is re-exported by
//! `registry.rs`.

use std::path::{Path, PathBuf};

use super::{BackupStepCallback, TestPostRunCallback};

impl super::OnlineBackupRegistry {
    /// Arm a one-shot step callback for this registry's NEXT `start`: the
    /// callback is moved onto that run's runner thread (and only that
    /// thread) before the protocol runs, and fires at each engine
    /// barrier-step boundary through the shared dispatcher
    /// ([`super::install_test_step_hook`]). Per-registry opt-in: no other run,
    /// registry instance or test observes it.
    pub(crate) fn arm_runner_step_callback(&self, callback: BackupStepCallback) {
        *self.pending_step_callback.lock() = Some(callback);
    }

    #[cfg(test)]
    pub(in crate::boot::config::diagnostics::backup_online) fn pending_adoption_count_for_test(
        &self,
        root: &Path,
        owner: &str,
    ) -> usize {
        self.record
            .lock()
            .pending_adoptions
            .recorded_for(root, owner)
            .len()
    }

    /// Arm a one-shot post-run callback for this registry's NEXT `start`:
    /// after the protocol returns `Ok` on the runner thread and before the
    /// guard stamp, completion capture and retention pass, it may rename the
    /// published child (`None` keeps the report path). Per-registry opt-in.
    pub(in crate::boot::config::diagnostics::backup_online) fn arm_post_run_callback(
        &self,
        callback: TestPostRunCallback,
    ) {
        *self.pending_post_run_callback.lock() = Some(callback);
    }

    /// Ordered-shutdown test seam: arm the one-shot post-run callback the NEXT
    /// `start` moves onto its runner thread.
    ///
    /// Generic on purpose: the crate-private callback type never becomes an API
    /// surface, and no existing item's visibility changes. A shutdown test parks
    /// the REAL runner here — after the protocol returned, before the completion
    /// signal — to prove the shutdown deadline does not abandon the worker.
    pub(crate) fn arm_post_run_callback_for_shutdown_test(
        &self,
        callback: impl Fn(&Path) -> Option<PathBuf> + Send + 'static,
    ) {
        self.arm_post_run_callback(Box::new(callback));
    }
}
