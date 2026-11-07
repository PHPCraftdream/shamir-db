use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_collections::THasher;
use shamir_connect::common::push_envelope::PushEnvelope;
use tokio::sync::{mpsc, oneshot};

use super::PushSender;
use crate::client::transport::ClosedState;
#[cfg(test)]
use crate::client::transport::HookPoint;
use crate::error::CloseCause;

pub(crate) const EARLY_BUFFER_CAP: usize = 256;

/// RV-CLIENT-D5B9F8A3: aggregate cap over ALL early-buffer entries combined
/// (not per key). The set of subscription ids is server-controlled, so
/// per-key caps alone leave total memory unbounded for the connection's
/// life; pushes past this total are dropped (with a `tracing::debug!`).
/// Budget frees naturally: `register` removes its key's buffer on
/// registration.
pub(crate) const EARLY_BUFFER_TOTAL_CAP: usize = 1024;

/// Per-subscription routing state. One map entry per subscription id, so
/// every transition is a single atomic per-key operation.
enum RouteState {
    /// Pushes that arrived before the subscription registered, in arrival
    /// order. Flushed to the registering channel under the entry guard.
    Early(VecDeque<PushEnvelope>),
    /// Registered route and its single-shot termination channel.
    Live {
        sender: PushSender,
        terminal: oneshot::Sender<CloseCause>,
    },
}

/// Lock-free subscription registry (RV-CLIENT-B80C0D09): replaces the
/// `std::sync::Mutex` that guarded live routes, early buffers, termination
/// channels and the aggregate early counter.
///
/// State machine per id: absent -> `Early` (first push before registration)
/// -> `Live` (registration, which flushes the early buffer in order before
/// any later push can be routed, because both run under the same entry
/// guard) -> absent (stale-checked handle drop, or shutdown drain).
///
/// The aggregate early budget is an exact counter reserved with a CAS loop
/// and released by exactly the path that removes the buffered items.
///
/// Close protocol (shared `ClosedState` with the pending-request registry):
/// the cause is published BEFORE the drain, and every mutating operation
/// re-reads the close flag AFTER its map transition. A transition that lands
/// before the drain visits its entry is resolved by the drain; one that
/// lands after removes its own entry. The flag store and load are `SeqCst`,
/// which rules out the store-buffering outcome in which the drain misses the
/// entry and the operation misses the flag. The drain is a traversal, not a
/// global snapshot; correctness comes from the post-operation re-read.
pub(crate) struct SubscriptionRegistry {
    map: scc::HashMap<u64, RouteState, THasher>,
    early_total: AtomicUsize,
    closed: Arc<ClosedState>,
}

pub(crate) type SubsMap = Arc<SubscriptionRegistry>;

impl SubscriptionRegistry {
    pub(crate) fn new(closed: Arc<ClosedState>) -> Self {
        Self {
            map: scc::HashMap::with_hasher(THasher::default()),
            early_total: AtomicUsize::new(0),
            closed,
        }
    }

    /// Reserve one slot of the aggregate early budget.
    fn reserve(&self) -> bool {
        self.early_total
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < EARLY_BUFFER_TOTAL_CAP).then_some(count + 1)
            })
            .is_ok()
    }

    fn release(&self, count: usize) {
        if count != 0 {
            let old = self.early_total.fetch_sub(count, Ordering::AcqRel);
            debug_assert!(old >= count, "early-buffer reservation underflow");
        }
    }

    fn send(&self, sender: &PushSender, env: PushEnvelope) {
        match sender.try_send(env) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(env)) => {
                tracing::warn!("client mpsc full for sub={}; dropping push", env.sub);
            }
            // Consumer dropped — SubscriptionHandle::Drop has or will clean
            // up the registry entry.
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    /// Route one push: to the live sender, or into the early buffer when the
    /// subscription has not registered yet.
    pub(crate) fn route(&self, env: PushEnvelope) {
        if self.route_precheck() {
            let id = env.sub;
            self.route_apply(env);
            self.route_cleanup(id);
        }
    }

    fn route_precheck(&self) -> bool {
        !self.closed.is_closed()
    }

    fn route_apply(&self, env: PushEnvelope) {
        let id = env.sub;
        match self.map.entry_sync(id) {
            scc::hash_map::Entry::Occupied(mut entry) => match entry.get_mut() {
                RouteState::Live { sender, .. } => self.send(sender, env),
                RouteState::Early(items) => {
                    if items.len() >= EARLY_BUFFER_CAP {
                        tracing::debug!(
                            "reader_task: early buffer full for sub {id}; push dropped"
                        );
                    } else if self.reserve() {
                        items.push_back(env);
                    } else {
                        tracing::debug!(
                            "reader_task: aggregate early buffer full; push for sub {id} dropped"
                        );
                    }
                }
            },
            scc::hash_map::Entry::Vacant(entry) => {
                if self.reserve() {
                    entry.insert_entry(RouteState::Early(VecDeque::from([env])));
                } else {
                    tracing::debug!(
                        "reader_task: aggregate early buffer full; push for sub {id} dropped"
                    );
                }
            }
        }
    }

    /// After a close, a buffer created or extended after the drain passed
    /// the key is removed here and its budget released.
    fn route_cleanup(&self, id: u64) {
        if !self.closed.is_closed() {
            return;
        }
        if let Some((_, RouteState::Early(items))) = self
            .map
            .remove_if_sync(&id, |state| matches!(state, RouteState::Early(_)))
        {
            self.release(items.len());
        }
    }

    /// Install the live route for `id`, flushing its early buffer in arrival
    /// order. `Err(cause)` when the client is closed: the termination channel
    /// has then been resolved with that cause and no route remains.
    pub(crate) fn register(
        &self,
        id: u64,
        sender: PushSender,
        terminal: oneshot::Sender<CloseCause>,
    ) -> Result<(), CloseCause> {
        let terminal = self.register_precheck(terminal)?;
        self.register_insert(id, &sender, terminal);
        self.register_postcheck(id, &sender)
    }

    fn register_precheck(
        &self,
        terminal: oneshot::Sender<CloseCause>,
    ) -> Result<oneshot::Sender<CloseCause>, CloseCause> {
        match self.closed.cause() {
            Some(cause) => {
                let _ = terminal.send(cause.clone());
                Err(cause)
            }
            None => Ok(terminal),
        }
    }

    fn register_insert(&self, id: u64, sender: &PushSender, terminal: oneshot::Sender<CloseCause>) {
        match self.map.entry_sync(id) {
            scc::hash_map::Entry::Vacant(entry) => {
                entry.insert_entry(RouteState::Live {
                    sender: sender.clone(),
                    terminal,
                });
            }
            scc::hash_map::Entry::Occupied(mut entry) => {
                let live = RouteState::Live {
                    sender: sender.clone(),
                    terminal,
                };
                // The flush runs under the entry guard: a concurrent `route`
                // for this id waits for the guard, so buffered pushes always
                // precede later live pushes.
                if let RouteState::Early(buffered) = std::mem::replace(entry.get_mut(), live) {
                    self.release(buffered.len());
                    for env in buffered {
                        self.send(sender, env);
                    }
                }
            }
        }
    }

    fn register_postcheck(&self, id: u64, sender: &PushSender) -> Result<(), CloseCause> {
        match self.closed.cause() {
            Some(cause) => {
                self.remove_owned(id, sender, Some(cause.clone()));
                Err(cause)
            }
            None => Ok(()),
        }
    }

    /// Remove the live route of `id` only if it is still `owner`'s channel,
    /// resolving its termination channel with `cause` when given.
    pub(crate) fn remove_owned(
        &self,
        id: u64,
        owner: &PushSender,
        cause: Option<CloseCause>,
    ) -> bool {
        self.map
            .remove_if_sync(&id, |state| {
                matches!(state, RouteState::Live { sender, .. } if sender.same_channel(owner))
            })
            .map(|(_, state)| {
                if let (RouteState::Live { terminal, .. }, Some(cause)) = (state, cause) {
                    let _ = terminal.send(cause);
                }
            })
            .is_some()
    }

    /// Terminal drain: resolves every termination channel with `cause`,
    /// drops every route and early buffer and returns their budget. Called
    /// once, after the cause was published.
    pub(crate) fn shutdown_drain(&self, cause: &CloseCause) {
        self.map.retain_sync(|_, state| {
            match state {
                RouteState::Live { terminal, .. } => {
                    // Move the sender out (oneshot::Sender is not Clone):
                    // leave a throwaway channel behind — the entry is
                    // removed anyway.
                    let (dummy, _) = oneshot::channel();
                    let _ = std::mem::replace(terminal, dummy).send(cause.clone());
                }
                RouteState::Early(items) => self.release(items.len()),
            }
            false
        });
    }

    #[cfg(test)]
    pub(crate) async fn route_test(
        &self,
        env: PushEnvelope,
        after_precheck: Option<&HookPoint>,
        after_apply: Option<&HookPoint>,
    ) {
        if self.route_precheck() {
            let id = env.sub;
            if let Some(hook) = after_precheck {
                hook.fire().await;
            }
            self.route_apply(env);
            if let Some(hook) = after_apply {
                hook.fire().await;
            }
            self.route_cleanup(id);
        }
    }

    #[cfg(test)]
    pub(crate) async fn register_test(
        &self,
        id: u64,
        sender: PushSender,
        terminal: oneshot::Sender<CloseCause>,
        before_insert: Option<&HookPoint>,
        after_insert: Option<&HookPoint>,
    ) -> Result<(), CloseCause> {
        let terminal = self.register_precheck(terminal)?;
        if let Some(hook) = before_insert {
            hook.fire().await;
        }
        self.register_insert(id, &sender, terminal);
        if let Some(hook) = after_insert {
            hook.fire().await;
        }
        self.register_postcheck(id, &sender)
    }

    #[cfg(test)]
    pub(crate) fn early_total(&self) -> usize {
        self.early_total.load(Ordering::Acquire)
    }

    /// Sum of the buffered envelopes actually held by the map; must equal
    /// [`Self::early_total`] whenever no operation is in flight.
    #[cfg(test)]
    pub(crate) fn debug_early_total(&self) -> usize {
        let mut sum = 0;
        self.map.iter_sync(|_, state| {
            if let RouteState::Early(items) = state {
                sum += items.len();
            }
            true
        });
        sum
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, id: &u64) -> bool {
        self.map.contains_sync(id)
    }
}
