use std::sync::Arc;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_db::access::Actor;
use shamir_db::ShamirDb;
use shamir_query_types::batch::{BatchOp, BatchRequest, BatchResponse};

use crate::subscriptions::registry::ActiveSubscription;
use crate::subscriptions::{bridge, SubscriptionRegistry};

/// Activates/deactivates subscriptions after a successful batch execute.
/// Called from handler.rs alongside persist_table_lifecycle.
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

    // Downcast extensions to SubscriptionRegistry.
    let registry = conn
        .extensions
        .as_ref()
        .and_then(|ext| ext.downcast_ref::<SubscriptionRegistry>());
    let Some(registry) = registry else {
        tracing::debug!("no subscription registry on connection");
        return;
    };

    for (alias, entry) in &batch.queries {
        match &entry.op {
            BatchOp::Subscribe(op) => {
                let sub_id = registry.next_id();
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
                ));
                registry.insert(
                    sub_id,
                    ActiveSubscription {
                        bridge_handle: handle,
                    },
                );
                if let Some(qr) = response.results.get_mut(alias) {
                    if let Some(serde_json::Value::Object(map)) = &mut qr.value {
                        map.insert("sub".to_string(), serde_json::Value::from(sub_id));
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
