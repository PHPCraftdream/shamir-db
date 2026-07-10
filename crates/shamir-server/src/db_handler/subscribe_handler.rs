use std::sync::Arc;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_db::access::Actor;
use shamir_db::ShamirDb;
use shamir_query_types::batch::{BatchOp, BatchRequest, BatchResponse};
use shamir_types::types::value::QueryValue;

use crate::subscriptions::{bridge, SubscriptionRegistry};

/// Activates/deactivates subscriptions after a successful batch execute.
/// Called from handler.rs alongside persist_table_lifecycle.
///
/// # CRIT-5 (#439): partial-reject delivery semantics
///
/// The client always receives a `sub_id` synchronously here, BEFORE
/// `bridge_task` runs the per-table read-ACL checks. The bridge silently
/// excludes any source the actor cannot `read` (see `bridge_task`). This
/// means a subscription that requested tables the actor has no `read` on
/// will deliver only the authorized subset — the client gets a `sub_id`
/// but fewer (or zero) event streams than requested.
///
/// A synchronous error to the client for the "all sources denied" case is
/// NOT possible here without a wider refactor: `authorize_access` is
/// `async`, but this function is synchronous. The bridge handles the
/// all-denied case by aborting (no receiver, no push). A future HIGH task
/// could surface this as a synchronous rejection if `activate_subscriptions`
/// is made async.
pub(super) fn activate_subscriptions(
    conn: &ConnectionServices,
    db: &Arc<ShamirDb>,
    db_name: &str,
    batch: &BatchRequest,
    response: &mut BatchResponse,
    actor: Actor,
) {
    let Some(push) = conn.push.as_ref() else {
        return; // No push channel — can't activate subscriptions.
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
        return;
    };

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
                registry.reserve_pending(sub_id);
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
                tracing::info!(sub_id, "subscription activated");
            }
            BatchOp::Unsubscribe(op) => {
                if registry.remove(op.unsubscribe) {
                    tracing::info!(sub_id = op.unsubscribe, "subscription deactivated");
                }
            }
            _ => {}
        }
    }
}
