//! Shared duplex-connection writer task.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::framer::{close_reason, CloseIntent, FrameWriter};

use super::super::request_loop::WriterMsg;

/// Drive heartbeat, replies, pushes, and coded connection shutdown identically for all transports.
pub(crate) async fn run_writer_task<W: FrameWriter>(
    writer: &mut W,
    rx: &mut mpsc::Receiver<WriterMsg>,
    heartbeat_for_writer: Option<shamir_transport_ws::HeartbeatState>,
    heartbeat_interval: Duration,
    heartbeat_timeout: Duration,
    close_intent: CloseIntent,
) {
    let mut next_ping = tokio::time::Instant::now() + heartbeat_interval;
    let mut deadline: Option<tokio::time::Instant> = None;
    let mut timer = Box::pin(tokio::time::sleep_until(next_ping));
    loop {
        tokio::select! {
            biased;
            _ = &mut timer, if heartbeat_for_writer.is_some() => {
                let state = heartbeat_for_writer.as_ref().expect("guarded heartbeat state");
                if let Some(deadline_at) = deadline {
                    if deadline_at <= tokio::time::Instant::now() && state.pending() && !state.is_reader_parked() {
                        let _ = tokio::time::timeout(Duration::from_secs(2), writer.close_with(4003, "heartbeat_timeout")).await;
                        return;
                    }
                    if state.is_reader_parked() {
                        deadline = Some(tokio::time::Instant::now() + heartbeat_timeout);
                        timer.as_mut().reset(deadline.expect("deadline set"));
                        continue;
                    }
                    deadline = None;
                    timer.as_mut().reset(next_ping);
                    continue;
                }
                let (payload, _) = state.next_ping();
                if writer.send_heartbeat(payload).await.is_err() { break; }
                let written = tokio::time::Instant::now();
                deadline = Some(written + heartbeat_timeout);
                next_ping = written + heartbeat_interval;
                timer.as_mut().reset(deadline.expect("deadline set"));
            },
            message = rx.recv() => match message {
                None => break,
                Some(WriterMsg::Reply(bytes, budget_guard)) => {
                    // DEFECT C fix: break immediately on write failure so
                    // the reader's select! wakes instead of hanging.
                    // RI-15: retain the budget guard across the write; it
                    // drops here on either success or error, releasing
                    // reserved bytes in the writer task.
                    let failed = writer.write_frame_prereserved(bytes).await.is_err();
                    drop(budget_guard);
                    if failed { break; }
                }
                Some(WriterMsg::Push(bytes)) => if writer.write_frame_prereserved(bytes).await.is_err() { break; },
                Some(WriterMsg::ReplyAndClose(bytes, budget_guard)) => {
                    // Write error is ignored; the connection is closing.
                    // RI-15: release the budget guard after the attempted
                    // write, whether it succeeds or fails.
                    let _ = writer.write_frame_prereserved(bytes).await;
                    drop(budget_guard);
                    break;
                }
            }
        }
    }
    match close_intent.take() {
        Some(code) => {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                writer.close_with(code, close_reason(code)),
            )
            .await;
        }
        None => {
            let _ = tokio::time::timeout(Duration::from_secs(2), writer.shutdown()).await;
        }
    }
}
