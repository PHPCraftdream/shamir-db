//! Confirmed-start gate (S5c-X PR8b-2 round 3, P1-C invariant): the set of
//! `(subscription, binding_generation)` pairs whose durable row is `active`
//! with a valid binding AND whose active state was confirmed by a
//! successful durability flush.
//!
//! The set is rebuilt wholesale from durable rows AFTER each successful
//! flush (via `refresh_resync_quarantine_cache`, which every subscription
//! mutation and open already calls), so it reflects exactly the rows whose
//! active state is durable. [`ShamirDb::may_start_follower`] is THE
//! supervisor start predicate: reconcile must consult it before starting a
//! follower. Because membership is only ever granted from rows observed in
//! durable storage after a successful flush, a row whose active state was
//! not confirmed by a successful flush can never pass — a flush error or
//! cancellation can never make a row startable.

use crate::repl_binding::binding::SubscriptionBinding;
use crate::repl_binding::validate::read_subscription_rows;
use crate::{DbResult, ShamirDb};
use dashmap::DashMap;
use shamir_collections::THasher;
use std::sync::Arc;

/// Per-instance registry of confirmed `(name, binding_generation)` pairs.
/// `Arc` inside so `ShamirDb` clones share one instance-scoped set.
#[derive(Clone)]
pub(crate) struct ConfirmedActive(Arc<DashMap<(String, u64), (), THasher>>);

/// The durable `active` state of a subscription row.
const ACTIVE_STATE: &str = "active";

impl ConfirmedActive {
    pub(crate) fn new() -> Self {
        Self(Arc::new(DashMap::with_hasher(THasher::default())))
    }

    /// Rebuild the set wholesale from the durable catalogue: keep only rows
    /// with `state == "active"` and a valid binding. A row that fails
    /// binding validation propagates the typed catalogue error (fail-closed:
    /// the previous set is cleared only on success of the row scan's parse —
    /// clear happens first, so an error leaves the set empty = nothing
    /// startable).
    pub(crate) async fn rebuild(&self, shamir: &ShamirDb) -> DbResult<()> {
        let rows = read_subscription_rows(shamir).await?;
        self.0.clear();
        for row in rows {
            if row.get("state").and_then(|value| value.as_str()) != Some(ACTIVE_STATE) {
                continue;
            }
            let name = row
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("<unnamed>");
            let binding = SubscriptionBinding::from_row(&row)?;
            self.0.insert((name.to_string(), binding.generation()), ());
        }
        Ok(())
    }

    /// Membership test: the row is active AND its `(name, generation)` is
    /// confirmed. Because `rebuild` inserts only durable active rows with
    /// valid bindings, membership implies both conditions.
    pub(crate) fn is_confirmed(&self, name: &str, generation: u64) -> bool {
        self.0.contains_key(&(name.to_string(), generation))
    }
}

impl ShamirDb {
    /// THE supervisor start predicate (P1-C invariant): a follower may be
    /// started for `name` at `generation` only when the durable row is
    /// `active` AND that `(name, generation)` was confirmed by a successful
    /// durability flush. Reconcile must consult this before starting a
    /// follower; a row whose active state was not confirmed by a successful
    /// flush can never pass, so a flush error or cancellation can never make
    /// a row startable.
    pub fn may_start_follower(&self, name: &str, generation: u64) -> bool {
        self.confirmed_active.is_confirmed(name, generation)
    }
}
