//! Token WebSocket writer that defers close frames until teardown completes.

use tokio_util::sync::CancellationToken;

use crate::framer::{FrameWriter, FramerError};

/// Defers writer-initiated close until the loop has synchronously cleaned up.
pub(crate) struct TokenWsCloseGuard<W> {
    writer: W,
    close_requested: CancellationToken,
    cleanup_complete: CancellationToken,
}

impl<W> TokenWsCloseGuard<W> {
    /// Wrap a writer with the token teardown barrier.
    pub(crate) fn new(
        writer: W,
        close_requested: CancellationToken,
        cleanup_complete: CancellationToken,
    ) -> Self {
        Self {
            writer,
            close_requested,
            cleanup_complete,
        }
    }
}

impl<W: FrameWriter> FrameWriter for TokenWsCloseGuard<W> {
    async fn write_frame_into(
        &mut self,
        payload: &[u8],
        scratch: &mut Vec<u8>,
    ) -> Result<(), FramerError> {
        self.writer.write_frame_into(payload, scratch).await
    }

    async fn write_frame_prereserved(&mut self, buf: Vec<u8>) -> Result<(), FramerError> {
        self.writer.write_frame_prereserved(buf).await
    }

    async fn close_with(&mut self, code: u16, reason: &str) {
        self.close_requested.cancel();
        self.cleanup_complete.cancelled().await;
        self.writer.close_with(code, reason).await;
    }

    fn heartbeat_state(&self) -> Option<shamir_transport_ws::HeartbeatState> {
        self.writer.heartbeat_state()
    }

    async fn send_heartbeat(&mut self, payload: [u8; 8]) -> Result<(), FramerError> {
        self.writer.send_heartbeat(payload).await
    }

    async fn shutdown(&mut self) {
        self.close_requested.cancel();
        self.cleanup_complete.cancelled().await;
        self.writer.shutdown().await;
    }
}
