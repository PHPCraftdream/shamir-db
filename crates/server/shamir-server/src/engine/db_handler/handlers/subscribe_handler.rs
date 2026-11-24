use std::sync::Arc;
use std::time::Duration;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_db::access::Actor;
use shamir_db::ShamirDb;
use shamir_query_types::batch::{BatchOp, BatchRequest, BatchResponse};
use shamir_types::types::value::QueryValue;

use crate::subscriptions::{bridge, SubscriptionRegistry};

/// Upper bound on how long a `Subscribe` ack waits for the bridge task to attach its changefeed
/// receivers. Normal attach takes milliseconds; the bound only protects the request from a
/// stuck bridge (the subscription then stays active and the ack goes out as before).
const ACTIVATION_WAIT: Duration = Duration::from_secs(5);

/// Activates/deactivates subscriptions after a successful batch execute.
/// Called from handler.rs alongside persist_table_lifecycle. Returns true if
/// any bridge failed the activation barrier; the request handler then replaces
/// the successful batch response with the fixed table-unavailable refusal.
/// Batch operations have already executed by then and are not rolled back.
///
/// # Activation barrier
///
/// The `sub_id` is not acknowledged to the client until each spawned bridge has
/// attached its changefeed receivers and sampled the activation cut (bounded by
/// [`ACTIVATION_WAIT`]). Without it the ack raced the attach: a write the client issued right
/// after the ack could commit BEFORE the cut and never be delivered, because a live-only
/// subscription starts at the cut.
pub(crate) async fn activate_subscriptions(
    conn: &ConnectionServices,
    db: &Arc<ShamirDb>,
    db_name: &str,
    batch: &BatchRequest,
    response: &mut BatchResponse,
    actor: Actor,
) -> bool {
    let Some(push) = conn.push.as_ref() else {
        return false; // No push channel — can't activate subscriptions.
    };

    // Downcast extensions to an OWNED, independently-cloneable
    // `Arc<SubscriptionRegistry>`: `extensions` is already an
    // `Arc<dyn Any + Send + Sync>`, so `Arc::downcast` yields an
    // `Arc<SubscriptionRegistry>` without any type changes. The owned Arc lets
    // us (a) drive the registry here and (b) hand a clone to each spawned
    // bridge task so it can release its own slot on self-exit (finding 2b /
    // former #513). On downcast failure the original Arc is returned in `Err`
    // and discarded.
    let Some(registry) = conn
        .extensions
        .clone()
        .and_then(|ext| ext.downcast::<SubscriptionRegistry>().ok())
    else {
        tracing::debug!("no subscription registry on connection");
        return false;
    };

    let mut ready_waiters = Vec::new();
    for (alias, entry) in &batch.queries {
        match &entry.op {
            BatchOp::Subscribe(op) => {
                // Per-connection subscription cap (finding 2b-i). Reserve a
                // slot BEFORE spawning the bridge; if the connection is
                // already at its cap, reject this Subscribe (surface the
                // rejection in the alias result) instead of spawning an
                // unbounded bridge task + broadcast receiver.
                if let Err(cap) = registry.try_reserve() {
                    if let Some(qr) = response.results.get_mut(alias) {
                        if let Some(QueryValue::Map(map)) = &mut qr.value {
                            map.insert(
                                "error".to_string(),
                                QueryValue::Str(format!(
                                    "subscription limit reached ({cap} per connection)"
                                )),
                            );
                        }
                    }
                    tracing::warn!(cap, "subscription rejected: per-connection limit reached");
                    continue;
                }
                let sub_id = registry.next_id();
                // Reserve a real (handle-less) map entry BEFORE spawning —
                // closes a race `@fl` review found where a fast-exiting
                // bridge task's self-cleanup guard could run before this
                // handler got around to inserting the entry, leaving a
                // permanently-dangling slot once the entry was inserted
                // afterward. See `SubscriptionRegistry::reserve_pending`.
                let Some(ready_rx) = registry.reserve_pending_ready(sub_id) else {
                    continue;
                };
                ready_waiters.push((sub_id, ready_rx));
                // Operator-configured query limits (finding 2b-ii): the batch
                // limits here are already capped to the operator maximums in
                // `handler::execute`. Reactive `Batch`/`Call` deliveries run
                // under the SUBSCRIBING actor's identity, so they must be
                // bounded by these same limits rather than `BatchLimits::default()`.
                let handle = tokio::spawn(bridge::bridge_task(
                    sub_id,
                    Arc::clone(db),
                    db_name.to_string(),
                    op.subscribe.clone(),
                    op.deliver.clone(),
                    actor.clone(),
                    Arc::clone(push),
                    op.from_version,
                    op.initial,
                    batch.limits.clone(),
                    Arc::clone(&registry),
                ));
                registry.attach_handle(sub_id, handle);
                if let Some(qr) = response.results.get_mut(alias) {
                    if let Some(QueryValue::Map(map)) = &mut qr.value {
                        map.insert("sub".to_string(), QueryValue::Int(sub_id as i64));
                    }
                }
                tracing::info!(
                    sub_id,
                    from_version = ?op.from_version,
                    initial = op.initial,
                    "subscription bridge spawned; ack waits for the activation barrier",
                );
            }
            BatchOp::Unsubscribe(op) => {
                let removed = registry.remove(op.unsubscribe);
                if removed {
                    tracing::info!(sub_id = op.unsubscribe, "subscription deactivated");
                }
            }
            _ => {}
        }
    }
    let activation_ids: Vec<u64> = ready_waiters.iter().map(|(sub_id, _)| *sub_id).collect();
    let mut activation_failed = false;
    for (sub_id, ready_rx) in ready_waiters {
        // `sub_id` remains monotonic, but a failed activation must not leave
        // a registry slot or a client-visible grant.
        let failed = match tokio::time::timeout(ACTIVATION_WAIT, ready_rx).await {
            Ok(Ok(())) => false,
            Ok(Err(_)) => {
                tracing::warn!(sub_id, "bridge exited before attaching; activation failed");
                true
            }
            Err(_) => {
                tracing::warn!(sub_id, "activation barrier timed out; activation failed");
                true
            }
        };
        if failed {
            activation_failed = true;
            // Remove every reservation/grant from this batch, not merely the failed alias.
            for reserved_id in &activation_ids {
                registry.remove(*reserved_id);
            }
            break;
        }
    }
    activation_failed
}
