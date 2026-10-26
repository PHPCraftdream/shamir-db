use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::server::conn_services::PushSink;
use shamir_db::access::Actor;
use shamir_db::shamir_db::ChangelogDeliveryGate;
use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;
use shamir_query_types::batch::BatchLimits;
use shamir_query_types::subscribe::deliver_mode::DeliverMode;
use shamir_tunables::instance_defaults::SLOW_CONSUMER_THRESHOLD;

use super::reactive::{execute_reactive_batch, execute_reactive_call};
use crate::subscriptions::payload::{make_event_data, make_keys_data};

/// Borrowing version of [`PushEnvelope`] used for serialization only.
///
/// Identical wire format — the `data` field borrows a `&[u8]` instead of
/// owning a `Vec<u8>`, eliminating a per-subscriber deep-copy of the
/// cached payload.
#[derive(Serialize)]
struct PushEnvelopeRef<'a> {
    push: PushKind,
    sub: u64,
    seq: u64,
    /// RV-CONNECT-60D4DAB1: bin encoding, matching `PushEnvelope`.
    #[serde(skip_serializing_if = "Option::is_none", with = "serde_bytes")]
    data: Option<&'a [u8]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gap_at: Option<u64>,
}

/// Build deliver payload for a single change, dispatching on `DeliverMode`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn make_deliver_data(
    deliver: &DeliverMode,
    db: &ShamirDb,
    db_name: &str,
    actor: &Actor,
    change: &shamir_tx::changefeed::RecordChange,
    value_qv: Option<&QueryValue>,
    commit_version: u64,
    query_limits: &BatchLimits,
) -> Vec<u8> {
    match deliver {
        DeliverMode::Records => make_event_data(change, value_qv, commit_version),
        DeliverMode::Keys => make_keys_data(&change.table, &change.op, &change.key, commit_version),
        DeliverMode::Batch(sub_batch) => {
            execute_reactive_batch(
                db,
                db_name,
                actor,
                sub_batch,
                change,
                commit_version,
                query_limits,
            )
            .await
        }
        DeliverMode::Call(call_op) => {
            execute_reactive_call(
                db,
                db_name,
                actor,
                call_op,
                change,
                commit_version,
                query_limits,
            )
            .await
        }
    }
}

// 100 attempts with 99 delays: at most 495 ms of scheduled retry sleep per
// required frame (executor scheduling can add latency). No commit-path wait.
const PUSH_RETRY_DELAY: Duration = Duration::from_millis(5);

/// Retry only while the delivery lease is open; sink acceptance and sequence
/// advancement are one synchronous lease operation, linearized with closure.
pub(crate) async fn push_required_gated(
    gate: &ChangelogDeliveryGate,
    push: &Arc<dyn PushSink>,
    sub_id: u64,
    seq: &AtomicU64,
    frame: Vec<u8>,
) -> bool {
    for attempt in 0..SLOW_CONSUMER_THRESHOLD {
        if let Some(accepted) = gate.try_with_delivery(|| push.try_push(frame.clone()).is_ok()) {
            if accepted {
                seq.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        } else {
            return false;
        }
        if attempt + 1 < SLOW_CONSUMER_THRESHOLD {
            tokio::select! {
                _ = gate.closed() => return false,
                () = tokio::time::sleep(PUSH_RETRY_DELAY) => {}
            }
        }
    }

    let slow_consumer = PushEnvelope {
        push: PushKind::SlowConsumer,
        sub: sub_id,
        seq: seq.load(Ordering::Relaxed),
        data: None,
        gap_at: None,
    };
    if let Ok(frame) = rmp_serde::to_vec_named(&slow_consumer) {
        if let Some(accepted) = gate.try_with_delivery(|| push.try_push(frame).is_ok()) {
            if accepted {
                seq.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    tracing::warn!(sub_id, "slow consumer - closing subscription");
    false
}

/// Serialize an Event and retry each sink acceptance under a delivery lease.
pub(crate) async fn try_push_event_gated(
    gate: &ChangelogDeliveryGate,
    push: &Arc<dyn PushSink>,
    sub_id: u64,
    seq: &AtomicU64,
    data: &[u8],
) -> bool {
    let envelope = PushEnvelopeRef {
        push: PushKind::Event,
        sub: sub_id,
        seq: seq.load(Ordering::Relaxed),
        data: Some(data),
        gap_at: None,
    };
    let frame = match rmp_serde::to_vec_named(&envelope) {
        Ok(frame) => frame,
        Err(error) => {
            tracing::warn!(sub_id, %error, "event serialization failed - closing subscription");
            return false;
        }
    };
    push_required_gated(gate, push, sub_id, seq, frame).await
}

/// Retain a serialized required frame until accepted, or close on exhaustion.
/// The caller must stop delivery on `false`; terminal notifications are best-effort.
/// cancel-safe: NO - cancellation discards the retained frame; only cancel when
/// terminating the subscription, never cancel and resume ordinary delivery.
#[cfg(test)]
pub(crate) async fn push_required(
    push: &Arc<dyn PushSink>,
    sub_id: u64,
    seq: &AtomicU64,
    frame: Vec<u8>,
) -> bool {
    for attempt in 0..SLOW_CONSUMER_THRESHOLD {
        // PushSink consumes the Vec even on rejection and returns no payload.
        // Keep the original serialized bytes; do not rebuild reactive delivery.
        if push.try_push(frame.clone()).is_ok() {
            seq.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        if attempt + 1 < SLOW_CONSUMER_THRESHOLD {
            tokio::time::sleep(PUSH_RETRY_DELAY).await;
        }
    }

    let sc = PushEnvelope {
        push: PushKind::SlowConsumer,
        sub: sub_id,
        seq: seq.load(Ordering::Relaxed),
        data: None,
        gap_at: None,
    };
    if let Ok(sc_frame) = rmp_serde::to_vec_named(&sc) {
        // Terminal notification only: rejection cannot keep the bridge alive.
        if push.try_push(sc_frame).is_ok() {
            seq.fetch_add(1, Ordering::Relaxed);
        }
    }
    tracing::warn!(sub_id, "slow consumer - closing subscription");
    false
}

/// Serialize an Event once and retry its frame; `false` terminates the bridge.
/// Seq advances only after acceptance, never for a rejected event.
/// cancel-safe: NO - cancellation must terminate the subscription, not resume it.
#[cfg(test)]
pub(crate) async fn try_push_event(
    push: &Arc<dyn PushSink>,
    sub_id: u64,
    seq: &AtomicU64,
    data: &[u8],
) -> bool {
    let envelope = PushEnvelopeRef {
        push: PushKind::Event,
        sub: sub_id,
        seq: seq.load(Ordering::Relaxed),
        data: Some(data),
        gap_at: None,
    };
    let frame = match rmp_serde::to_vec_named(&envelope) {
        Ok(frame) => frame,
        Err(error) => {
            tracing::warn!(sub_id, %error, "event serialization failed - closing subscription");
            return false;
        }
    };
    push_required(push, sub_id, seq, frame).await
}
