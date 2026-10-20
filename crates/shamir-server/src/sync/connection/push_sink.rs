use shamir_connect::server::conn_services::{PushRejected, PushSink};
use tokio::sync::mpsc;

use super::request_loop::{prereserve_frame, WriterMsg};

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
        // §3.4: prereserve the frame ([4-byte len][payload]) so the writer
        // can call `write_frame_prereserved` directly without a memcpy.
        let buf = prereserve_frame(&frame);
        self.tx
            .try_send(WriterMsg::Push(buf))
            .map_err(|_| PushRejected)
    }
}
