use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_connect::common::push_envelope::PushEnvelope;
use shamir_connect::server::conn_services::{PushRejected, PushSink};
use tokio::sync::{mpsc, Notify};

/// First accepted event is synchronized; later pushes stay rejected until released.
pub(super) struct PausingSink {
    pub frames: mpsc::Sender<PushEnvelope>,
    pub event_attempts: AtomicUsize,
    pub entered: Notify,
    pub allow_after_revoke: AtomicBool,
}

impl PausingSink {
    pub fn new(frames: mpsc::Sender<PushEnvelope>) -> Arc<Self> {
        Arc::new(Self {
            frames,
            event_attempts: AtomicUsize::new(0),
            entered: Notify::new(),
            allow_after_revoke: AtomicBool::new(false),
        })
    }
}

impl PushSink for PausingSink {
    fn try_push(&self, frame: Vec<u8>) -> Result<(), PushRejected> {
        let envelope: PushEnvelope = rmp_serde::from_slice(&frame).unwrap();
        if envelope.push == shamir_connect::common::push_envelope::PushKind::Event {
            let n = self.event_attempts.fetch_add(1, Ordering::AcqRel);
            if n > 0 && !self.allow_after_revoke.load(Ordering::Acquire) {
                self.entered.notify_one();
                return Err(PushRejected);
            }
        }
        self.frames.try_send(envelope).map_err(|_| PushRejected)
    }
}
