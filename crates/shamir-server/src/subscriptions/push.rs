use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use shamir_connect::server::conn_services::PushSink;
use shamir_db::access::Actor;
use shamir_db::ShamirDb;
use shamir_query_types::subscribe::deliver_mode::DeliverMode;
use shamir_tunables::instance_defaults::SLOW_CONSUMER_THRESHOLD;

use super::payload::{make_event_data, make_keys_data};
use super::reactive::{execute_reactive_batch, execute_reactive_call};

/// Build deliver payload for a single change, dispatching on `DeliverMode`.
pub(super) async fn make_deliver_data(
    deliver: &DeliverMode,
    db: &ShamirDb,
    db_name: &str,
    actor: &Actor,
    change: &shamir_tx::changefeed::RecordChange,
    value_json: Option<&serde_json::Value>,
    commit_version: u64,
) -> Vec<u8> {
    match deliver {
        DeliverMode::Records => make_event_data(change, value_json, commit_version),
        DeliverMode::Keys => make_keys_data(&change.table, &change.op, &change.key, commit_version),
        DeliverMode::Batch(sub_batch) => {
            execute_reactive_batch(db, db_name, actor, sub_batch, change, commit_version).await
        }
        DeliverMode::Call(call_op) => {
            execute_reactive_call(db, db_name, actor, call_op, change, commit_version).await
        }
    }
}

/// Serialize and push an Event envelope. Returns `false` when the slow-consumer
/// threshold is hit and the bridge should shut down.
///
/// Seq is only advanced on successful delivery so the client never observes
/// a seq hole from transient backpressure drops.
pub(super) fn try_push_event(
    push: &Arc<dyn PushSink>,
    sub_id: u64,
    seq: &AtomicU64,
    data: Vec<u8>,
    consecutive_push_failures: &mut u32,
) -> bool {
    let s = seq.load(Ordering::Relaxed);
    let envelope = PushEnvelope {
        push: PushKind::Event,
        sub: sub_id,
        seq: s,
        data: Some(data),
        gap_at: None,
    };
    let frame = match rmp_serde::to_vec_named(&envelope) {
        Ok(b) => b,
        Err(_) => return true, // serialization error — skip, not fatal
    };
    if push.try_push(frame).is_err() {
        *consecutive_push_failures += 1;
        if *consecutive_push_failures >= SLOW_CONSUMER_THRESHOLD {
            let sc_seq = seq.fetch_add(1, Ordering::Relaxed);
            let sc = PushEnvelope {
                push: PushKind::SlowConsumer,
                sub: sub_id,
                seq: sc_seq,
                data: None,
                gap_at: None,
            };
            if let Ok(sc_frame) = rmp_serde::to_vec_named(&sc) {
                let _ = push.try_push(sc_frame);
            }
            tracing::warn!(sub_id, "slow consumer — closing subscription");
            return false;
        }
        tracing::debug!(sub_id, "push rejected, event dropped");
    } else {
        *consecutive_push_failures = 0;
        seq.fetch_add(1, Ordering::Relaxed);
    }
    true
}
