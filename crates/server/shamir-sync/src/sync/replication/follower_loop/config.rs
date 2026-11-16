use std::sync::Arc;

use shamir_db::repl_binding::SubscriptionLease;

#[cfg(test)]
use super::test_hooks::ApplyParkHook;

/// Configuration for [`run_follower_loop`]: per-repo identity + tunables,
/// grouped so the loop entry point stays under the argument-count lint and
/// callers can build the config once and reuse it across restarts.
///
/// `Debug` is implemented manually because the lease (an `Arc` over the
/// db-side [`SubscriptionLease`], which carries a `tokio::sync::RwLock`) has
/// no `Debug` impl of its own in `shamir-db`.
#[derive(Clone)]
pub struct FollowerLoopConfig {
    /// Stable identity advertised in `ReplHello`.
    pub node_id: String,
    /// Target database on both leader and follower (must match).
    pub db: String,
    /// Target repo on both leader and follower (must match).
    pub repo: String,
    /// Long-poll budget forwarded to each `pull` (ms). The leader blocks
    /// up to this long waiting for new events before returning an empty
    /// batch (REPLICATION §5.1).
    pub poll_wait_ms: u32,
    pub validate_replicated_payload: bool,
    /// S5c-X PR7: the universal leader-identity fence on every Hello/Pull.
    /// `true` in production; tests may disable it through this explicit seam
    /// to exercise the unprotected path (negative control).
    #[cfg(test)]
    pub(crate) identity_fence: bool,
    /// Test/bound hook: if `Some(n)`, the loop exits cleanly after `n`
    /// iterations. `None` = run forever (until `cancel` fires). Production
    /// callers pass `None`; tests pass a small cap to avoid relying on
    /// cancellation timing.
    pub max_iterations: Option<usize>,
    /// S5c-X round 3: the subscription this loop serves. Empty means
    /// "unsupervised loop" (direct/test callers) with no lease — the
    /// lease gates below are skipped entirely — and stall visibility is
    /// disabled. A non-empty name enables PR6b-3 stall visibility: when the
    /// loop swallows [`STALL_VISIBILITY_THRESHOLD`] consecutive failures
    /// without bookmark progress, that subscription row is marked with a
    /// persistent `status_message` (the supervisor always sets it).
    pub subscription: String,
    /// S5c-X round 4: the per-subscription generation lease — the loop
    /// applies only while the lease is valid. `None` for unsupervised
    /// loops (empty [`Self::subscription`]).
    pub lease: Option<Arc<SubscriptionLease>>,
    /// Test-only apply park for selected target loops.
    #[cfg(test)]
    pub(crate) apply_park: Option<Arc<ApplyParkHook>>,
    /// Per-loop exit notification for test observability.
    #[cfg(test)]
    pub(crate) exit_hook: Option<Arc<ApplyParkHook>>,
    /// Test-only isolated registry; production loops always use the process registry.
    #[cfg(test)]
    pub(super) metrics_registry: Option<Arc<super::metrics::ReplicationRegistry>>,
}

impl std::fmt::Debug for FollowerLoopConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FollowerLoopConfig")
            .field("node_id", &self.node_id)
            .field("db", &self.db)
            .field("repo", &self.repo)
            .field("poll_wait_ms", &self.poll_wait_ms)
            .field(
                "validate_replicated_payload",
                &self.validate_replicated_payload,
            )
            .field("max_iterations", &self.max_iterations)
            .field("subscription", &self.subscription)
            .field(
                "lease",
                &self
                    .lease
                    .as_ref()
                    .map(|l| format!("generation={}", l.generation)),
            )
            .finish_non_exhaustive()
    }
}

impl FollowerLoopConfig {
    /// Build a config for one `(db, repo)` follower with the given poll
    /// budget and NO iteration cap (run until cancelled).
    pub fn new(node_id: impl Into<String>, db: impl Into<String>, repo: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            db: db.into(),
            repo: repo.into(),
            poll_wait_ms: 5000,
            validate_replicated_payload: false,
            #[cfg(test)]
            identity_fence: true,
            max_iterations: None,
            subscription: String::new(),
            lease: None,
            #[cfg(test)]
            apply_park: None,
            #[cfg(test)]
            exit_hook: None,
            #[cfg(test)]
            metrics_registry: None,
        }
    }

    /// Override the long-poll budget (ms).
    #[must_use]
    pub fn with_poll_wait_ms(mut self, ms: u32) -> Self {
        self.poll_wait_ms = ms;
        self
    }

    #[must_use]
    pub fn with_validate_replicated_payload(mut self, validate: bool) -> Self {
        self.validate_replicated_payload = validate;
        self
    }

    /// Disable the universal identity fence (negative-control tests only).
    #[must_use]
    #[cfg(test)]
    pub(crate) fn with_identity_fence(mut self, enabled: bool) -> Self {
        self.identity_fence = enabled;
        self
    }

    /// Set the iteration cap (mainly for tests).
    #[must_use]
    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = Some(n);
        self
    }

    /// Use a per-test owner registry rather than shared process telemetry.
    #[cfg(test)]
    pub(super) fn with_metrics_registry(
        mut self,
        registry: Arc<super::metrics::ReplicationRegistry>,
    ) -> Self {
        self.metrics_registry = Some(registry);
        self
    }

    /// Set the subscription row to mark on a no-progress stall
    /// ([`STALL_VISIBILITY_THRESHOLD`] swallowed failures without bookmark
    /// progress). The supervisor wires this; direct callers may omit it.
    #[must_use]
    pub fn with_subscription(mut self, name: impl Into<String>) -> Self {
        self.subscription = name.into();
        self
    }
}
