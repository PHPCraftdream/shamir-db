use shamir_connect::server::conn_services::{PushRejected, PushSink};
use tokio::sync::mpsc;

use super::request_loop::WriterMsg;

/// Adapts the writer's mpsc channel into a [`PushSink`] for subscription bridges.
pub(crate) struct MpscPushSink {
    tx: mpsc::Sender<WriterMsg>,
}

impl MpscPushSink {
    pub fn new(tx: mpsc::Sender<WriterMsg>) -> Self {
        Self { tx }
    }
}

impl PushSink for MpscPushSink {
    fn try_push(&self, frame: Vec<u8>) -> Result<(), PushRejected> {
        self.tx
            .try_send(WriterMsg::Push(frame))
            .map_err(|_| PushRejected)
    }
}
