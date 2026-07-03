//! Replication DDL operations — publication/subscription model (REPLICATION.md §5.5).
//!
//! These are **pure wire/serde DTOs**: they declare *what* the operator wants
//! (create a publication, subscribe to an upstream, pause a subscription,
//! inspect status). The server-side *execution* of these ops is a separate
//! concern (R1-loop integration) and is NOT implemented here — only the type
//! shapes, their serde contracts, and the `BatchOp`-level classification
//! (`is_admin` / `is_write`).
//!
//! ## Wire discriminator keys
//!
//! Each op carries a unique top-level key (the first named field) used by
//! [`BatchOp`](crate::batch::BatchOp) deserialization for dispatch, exactly
//! like the other admin ops (`create_db`, `start_migration`, …):
//!
//! | Discriminator | Op struct |
//! |---|---|
//! | `create_replication_profile` | [`CreateReplicationProfileOp`] |
//! | `drop_replication_profile`   | [`DropReplicationProfileOp`] |
//! | `create_publication`         | [`CreatePublicationOp`] |
//! | `drop_publication`           | [`DropPublicationOp`] |
//! | `create_subscription`        | [`CreateSubscriptionOp`] |
//! | `drop_subscription`          | [`DropSubscriptionOp`] |
//! | `alter_subscription`         | [`AlterSubscriptionOp`] |
//! | `list_publications`          | [`ListPublicationsOp`] |
//! | `list_subscriptions`         | [`ListSubscriptionsOp`] |
//! | `replication_status`         | [`ReplicationStatusOp`] |
//!
//! `list_publications` / `list_subscriptions` / `replication_status` are
//! read-only introspection ops (`is_write == false`); all the create/drop/alter
//! ops are write-classified (`is_write == true`). Every op here is an admin
//! op (`is_admin == true`) — repl-DDL is an administrative surface.

use serde::{Deserialize, Serialize};

/// Serde skip-serializing-if helper: omit `false` booleans from the wire.
///
/// Mirrors [`super::db_ops::is_false`] — declared locally to keep this
/// module self-contained (the read-only introspection ops below use it to
/// make a default-constructed payload byte-identical to an empty map, so
/// `ListPublicationsOp::default()` does not emit a spurious
/// `"list_publications": false` key).
pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

/// Replication scope — the `(db[, repo[, table]])` triple that identifies
/// *what* is replicated. `repo == None` means "the whole database";
/// `table == None` means "the whole repository".
///
/// Wire shape (msgpack / JSON):
/// ```text
/// { "db": "app", "repo": "main", "table": "users" }
/// { "db": "app", "repo": "edge_42" }       // whole repo
/// { "db": "system" }                        // whole db
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplScope {
    pub db: String,
    /// Repository inside `db`. `None` → every repo in the database.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Table inside `repo`. `None` → every table in the repository.
    /// Must be `None` when `repo` is `None` (a bare-database scope has no
    /// table granularity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
}

/// Replication direction, relative to the node that owns the profile stream.
///
/// See REPLICATION.md §5.5: R1 ships `Pull` only; `Push` (edge-collect) and
/// `Both` (R4, CRDT) are declared now so the wire contract is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReplDirection {
    /// Pull from upstream (R0/R1 — the only implemented direction).
    #[default]
    Pull,
    /// Push to upstream — edge-collect topology (R2+).
    Push,
    /// Bidirectional — peer / CRDT (R4).
    Both,
}

/// Replication access mode for a stream.
///
/// `ReadOnly` is the R1 default (a follower only applies upstream writes).
/// `ReadWrite` is declared for the edge-collect case where the local node
/// is itself a writer for the pushed scope (REPLICATION.md §5.5 "rw/ro —
/// свойство пары (узел, repo)").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReplMode {
    /// Read-only follower (R1 default).
    #[default]
    ReadOnly,
    /// Read-write — local node may also write the scope (edge-collect R2+).
    ReadWrite,
}

/// One `(scope, direction, mode)` rule inside a [`CreateReplicationProfileOp`].
///
/// A *stream* is the atomic unit of replication policy: it says "replicate
/// this scope, in this direction, with this access mode".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplStream {
    pub scope: ReplScope,
    #[serde(default)]
    pub direction: ReplDirection,
    #[serde(default)]
    pub mode: ReplMode,
}

/// Create a named replication-profile template: a bundle of
/// [`ReplStream`] rules. Profiles are stored in the system store of the
/// leader and are themselves replicated, so every cluster node sees the same
/// definitions (REPLICATION.md §5.5).
///
/// Wire shape:
/// ```text
/// {
///   "create_replication_profile": "cluster",
///   "streams": [
///     { "scope": { "db": "app" }, "direction": "pull", "mode": "read_only" }
///   ]
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateReplicationProfileOp {
    /// Profile name (discriminator key).
    pub create_replication_profile: String,
    pub streams: Vec<ReplStream>,
}

/// Drop a named replication profile. Accounts bound to the profile keep
/// running with their last-resolved rules until reassigned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropReplicationProfileOp {
    /// Profile name (discriminator key).
    pub drop_replication_profile: String,
}

/// Declare what the leader publishes: a set of [`ReplScope`]s that
/// downstream subscribers may pull. `system/*` (users, roles, settings) is
/// included by adding its scope explicitly — there is no separate
/// "replicate accounts" op (§5.5 insight).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatePublicationOp {
    /// Publication name (discriminator key).
    pub create_publication: String,
    pub scopes: Vec<ReplScope>,
}

/// Drop a publication. Active subscriptions to it become stale and stop
/// receiving new events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropPublicationOp {
    /// Publication name (discriminator key).
    pub drop_publication: String,
}

/// Subscribe this node to a remote publication, bound to a local
/// replication profile that governs direction/mode per scope.
///
/// `upstream` is an opaque identifier for the leader (address / cluster-id /
/// connection handle — the concrete form is finalized with R1's transport).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateSubscriptionOp {
    /// Subscription name (discriminator key).
    pub create_subscription: String,
    /// Address / identifier of the upstream leader.
    pub upstream: String,
    /// Name of the publication to subscribe to on the upstream.
    pub publication: String,
    /// Local replication profile governing this subscription's streams.
    pub profile: String,
}

/// Drop a subscription. The node stops pulling from the upstream; already-
/// applied data is unaffected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropSubscriptionOp {
    /// Subscription name (discriminator key).
    pub drop_subscription: String,
}

/// Action taken by [`AlterSubscriptionOp`] on an existing subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAction {
    /// Pause pulling — the subscription stays defined but no new events are
    /// fetched until `Resume`.
    Pause,
    /// Resume a paused subscription.
    Resume,
    /// Rebind the subscription to a different replication profile.
    SetProfile(String),
}

/// Alter an existing subscription: pause, resume, or rebind to a different
/// profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlterSubscriptionOp {
    /// Subscription name (discriminator key).
    pub alter_subscription: String,
    pub action: SubAction,
}

/// List all publications defined on this node (read-only introspection).
///
/// Wire form: `{ "list_publications": true }`. The discriminator is a
/// presence-only boolean flag (same convention as `access_tree`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ListPublicationsOp {
    /// Discriminator flag — presence-only, conventionally `true`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub list_publications: bool,
}

/// List all subscriptions defined on this node (read-only introspection).
///
/// Wire form: `{ "list_subscriptions": true }`. The discriminator is a
/// presence-only boolean flag (same convention as `access_tree`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ListSubscriptionsOp {
    /// Discriminator flag — presence-only, conventionally `true`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub list_subscriptions: bool,
}

/// Inspect the runtime replication status of this node: active subscriptions,
/// upstream connectivity, applied LSNs (read-only introspection).
///
/// Wire form: `{ "replication_status": true }`. The discriminator is a
/// presence-only boolean flag (same convention as `access_tree`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ReplicationStatusOp {
    /// Discriminator flag — presence-only, conventionally `true`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub replication_status: bool,
}
