use tokio_util::sync::CancellationToken;

/// Cancels the launcher task tree unless server startup completes successfully.
pub(in crate::engine::server::server_launcher) struct BootGuard {
    shutdown: CancellationToken,
    armed: bool,
}

impl BootGuard {
    pub(in crate::engine::server::server_launcher) fn new(shutdown: CancellationToken) -> Self {
        Self {
            shutdown,
            armed: true,
        }
    }

    pub(in crate::engine::server::server_launcher) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BootGuard {
    fn drop(&mut self) {
        if self.armed {
            self.shutdown.cancel();
        }
    }
}
