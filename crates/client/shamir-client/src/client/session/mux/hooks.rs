//! Test-only hook seam for the rid-demux mux (moved out of
//! `client/transport.rs`).

#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex as StdMutex;
#[cfg(test)]
use tokio::sync::mpsc;
#[cfg(test)]
use tokio::sync::oneshot;

/// One-shot test hook point: production code FIRES it (sends `reached`, then
/// awaits `release`); the test awaits `reached` and holds `release`.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct HookPoint {
    reached: Arc<StdMutex<Option<oneshot::Sender<()>>>>,
    release: Arc<StdMutex<Option<oneshot::Receiver<()>>>>,
    /// Multi-fire channel (see [`HookPoint::new_multi`]).
    multi: Option<mpsc::UnboundedSender<()>>,
}

#[cfg(test)]
impl HookPoint {
    /// Multi-fire variant (test oracle quality): every `fire()` sends a
    /// `reached` signal and NEVER blocks (release side unarmed). Returns
    /// `(point, reached_rx)`; the test awaits exactly `expected_fires`
    /// signals.
    pub(crate) fn new_multi(expected_fires: usize) -> (Self, mpsc::UnboundedReceiver<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        debug_assert!(expected_fires > 0, "arm with a positive fire count");
        (
            Self {
                reached: Arc::new(StdMutex::new(None)),
                release: Arc::new(StdMutex::new(None)),
                multi: Some(tx),
            },
            rx,
        )
    }

    /// Create an armed hook: `(point, reached_rx, release_tx)`.
    pub(crate) fn new() -> (Self, oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        (
            Self {
                reached: Arc::new(StdMutex::new(Some(reached_tx))),
                release: Arc::new(StdMutex::new(Some(release_rx))),
                multi: None,
            },
            reached_rx,
            release_tx,
        )
    }

    /// Fired by the production path: signal `reached`, then block until the
    /// test releases (or drops `release_tx`). Never blocks when unarmed or
    /// in multi mode.
    pub(crate) async fn fire(&self) {
        if let Some(tx) = self
            .reached
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = tx.send(());
        }
        if let Some(multi) = &self.multi {
            let _ = multi.send(());
        }
        // Take the receiver OUT of the lock before awaiting: the guard must
        // never be held across the `.await`.
        let rx = self
            .release
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(rx) = rx {
            let _ = rx.await;
        }
    }
}
