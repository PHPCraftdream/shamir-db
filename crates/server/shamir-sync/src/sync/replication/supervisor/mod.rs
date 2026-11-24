//! 386-b — [`SubscriptionSupervisor`]: bind the declarative replication
//! catalogue (386-a) to the follower engine (R1-c).
//!
//! An **active** subscription row in `system/subscriptions` (persisted by
//! `admin_replication.rs`) must translate into a running
//! [`run_follower_loop`](super::follower_loop::run_follower_loop) pulling from
//! the subscription's `upstream` and applying onto the local `(db, repo)`
//! destinations recorded in the row's DURABLE binding (S5c-X PR8b-2) — the
//! bound profile's `pull` streams are only checked for coverage, never used
//! as the target source. `pause` / `resume` /
//! `drop` (via `alter_subscription` / `drop_subscription`, 386-a) flip the
//! row's `state`; the supervisor's [`reconcile`](Self::reconcile) converges
//! the running loops to that declarative state.
//!
//! # Destination ownership and terminal state
//!
//! Catalogue lifecycle operations serialize under an async cold-path mutex.
//! Each registered destination has one owner, including while its stopped task
//! is being joined. No concurrent-map guard crosses an await. Terminal apply
//! failures persist `reopen_required`; admission refuses a still-dead gate.
//!
//! # Registry (lock-free, §ideology)
//!
//! Active loops live in a [`scc::HashMap`] keyed by subscription name, hashed
//! with the workspace [`THasher`]. Each entry owns a
//! [`CancellationToken`] (stop signal) + the spawned [`JoinHandle`]s (one per
//! bound destination) + the profile name and binding generation it was
//! started against (so a profile rebind or a replaced binding can be
//! detected and the loop restarted).
//!
//! # Reactivity — reconcile + `notify_changed`, NOT changefeed-watch (yet)
//!
//! For R1 the supervisor is **reconcile-driven**: [`reconcile`](Self::reconcile)
//! reads the catalogue and converges. It is idempotent, so it is safe to call
//! at boot and again after any admin batch that may have touched the
//! catalogue via [`notify_changed`](Self::notify_changed). A fully
//! event-driven watch on the `system` repo changefeed (REPLICATION §5.6) is
//! the intended end-state but is deferred:
//!
//! TODO(386-c): subscribe to the `system` repo changelog (table
//! `subscriptions`) and call `reconcile()` on each event, replacing the
//! explicit `notify_changed()` pokes with an event-driven watch.
//!
//! # §5.6 — non-blocking
//!
//! Every follower loop runs on its own [`tokio::spawn`] task; the supervisor
//! holds no follower locks and never blocks the server. `reconcile` only
//! reads the catalogue and starts/stops tasks.

mod handle;
mod machine;
mod start;

pub use super::source::seed::terminal::{ReplSourceFactory, Subscription};
#[cfg(test)]
pub(crate) use machine::HousekeepingGate;
pub use machine::SubscriptionSupervisor;
