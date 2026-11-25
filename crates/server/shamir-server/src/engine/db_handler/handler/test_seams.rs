use std::sync::Arc;

use super::ShamirDbHandler;

pub(super) type CommitPause = (u64, Arc<tokio::sync::Barrier>, Arc<tokio::sync::Barrier>);

type BeginPauseBarriers = (Arc<tokio::sync::Barrier>, Arc<tokio::sync::Barrier>);

#[derive(Default, Clone)]
pub(in crate::engine::db_handler) struct BeginPause(
    Arc<std::sync::Mutex<Option<BeginPauseBarriers>>>,
);

impl BeginPause {
    pub(in crate::engine::db_handler) fn install(
        &self,
        reached: Arc<tokio::sync::Barrier>,
        resume: Arc<tokio::sync::Barrier>,
    ) {
        *self.0.lock().expect("begin pause mutex poisoned") = Some((reached, resume));
    }

    pub(in crate::engine::db_handler) async fn pause(&self) {
        let pause = self.0.lock().expect("begin pause mutex poisoned").take();
        if let Some((reached, resume)) = pause {
            reached.wait().await;
            resume.wait().await;
        }
    }
}

impl ShamirDbHandler {
    /// Reads the raw-native-path decode counter (test-only reader).
    #[cfg(test)]
    pub(in crate::engine::db_handler) fn decoded_request_count(&self) -> u64 {
        self.decoded_requests
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Install a one-shot pause before this handler's cursor removal.
    pub(in crate::engine::db_handler) fn with_cursor_remove_pause(
        self,
        id: u64,
        reached: Arc<tokio::sync::Barrier>,
        resume: Arc<tokio::sync::Barrier>,
    ) -> Self {
        self.cursor_registry.with_remove_pause(id, reached, resume);
        self
    }

    /// Install this handler instance's one-shot pause after taking a commit context.
    pub(in crate::engine::db_handler) fn with_commit_handoff_pause(
        self,
        tx_handle: u64,
        reached: Arc<tokio::sync::Barrier>,
        resume: Arc<tokio::sync::Barrier>,
    ) -> Self {
        *self
            .commit_handoff_pause
            .lock()
            .expect("commit pause mutex poisoned") = Some((tx_handle, reached, resume));
        self
    }

    /// Pause the matching commit once, then clear the per-handler seam.
    pub(in crate::engine::db_handler) async fn pause_commit_after_take(&self, tx_handle: u64) {
        let pause = {
            let mut configured = self
                .commit_handoff_pause
                .lock()
                .expect("commit pause mutex poisoned");
            if configured
                .as_ref()
                .is_some_and(|(configured_handle, _, _)| *configured_handle == tx_handle)
            {
                configured.take()
            } else {
                None
            }
        };
        if let Some((_, reached, resume)) = pause {
            reached.wait().await;
            resume.wait().await;
        }
    }
}
