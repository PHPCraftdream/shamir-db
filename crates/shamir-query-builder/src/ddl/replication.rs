//! Replication DDL builders — publication/subscription model.
//!
//! Fluent constructors for the 10 replication `BatchOp` variants declared in
//! [`shamir_query_types::admin::types::repl_ops`]. Every mutating op returns a
//! builder struct with optional fields; read-only introspection ops (`list_*`,
//! `replication_status`) build directly into a `BatchOp` since they take no
//! arguments.

use shamir_query_types::admin::{
    AlterSubscriptionOp, CreatePublicationOp, CreateReplicationProfileOp, CreateSubscriptionOp,
    DropPublicationOp, DropReplicationProfileOp, DropSubscriptionOp, ListPublicationsOp,
    ListSubscriptionsOp, ReplicationStatusOp, SubAction,
};
use shamir_query_types::admin::{ReplDirection, ReplMode, ReplScope, ReplStream};
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

// ============================================================================
// ReplScope helper
// ============================================================================

/// Begin building a [`ReplScope`] for a given database.
///
/// `db` is mandatory; narrow further with `.repo()` / `.table()`.
pub fn repl_scope(db: impl Into<String>) -> ReplScopeBuilder {
    ReplScopeBuilder {
        db: db.into(),
        repo: None,
        table: None,
    }
}

/// Builder for [`ReplScope`].
pub struct ReplScopeBuilder {
    db: String,
    repo: Option<String>,
    table: Option<String>,
}

impl ReplScopeBuilder {
    /// Narrow the scope to a repository inside the database.
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = Some(repo.into());
        self
    }

    /// Narrow the scope to a table inside the repository (requires `repo`).
    pub fn table(mut self, table: impl Into<String>) -> Self {
        self.table = Some(table.into());
        self
    }

    /// Finalize the scope.
    pub fn build(self) -> ReplScope {
        ReplScope {
            db: self.db,
            repo: self.repo,
            table: self.table,
        }
    }
}

impl From<ReplScopeBuilder> for ReplScope {
    fn from(b: ReplScopeBuilder) -> Self {
        b.build()
    }
}

// ============================================================================
// create_replication_profile
// ============================================================================

/// Create a named replication-profile template bundling a set of stream rules.
pub fn replication_profile(name: impl Into<String>) -> ReplicationProfileBuilder {
    ReplicationProfileBuilder {
        name: name.into(),
        streams: Vec::new(),
    }
}

/// Builder for [`CreateReplicationProfileOp`].
pub struct ReplicationProfileBuilder {
    name: String,
    streams: Vec<ReplStream>,
}

impl ReplicationProfileBuilder {
    /// Add a `(scope, direction, mode)` stream rule.
    pub fn stream(mut self, scope: ReplScope, direction: ReplDirection, mode: ReplMode) -> Self {
        self.streams.push(ReplStream {
            scope,
            direction,
            mode,
        });
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateReplicationProfile(CreateReplicationProfileOp {
            create_replication_profile: self.name,
            streams: self.streams,
        })
    }
}

impl From<ReplicationProfileBuilder> for BatchOp {
    fn from(b: ReplicationProfileBuilder) -> Self {
        b.build()
    }
}

impl IntoBatchOp for ReplicationProfileBuilder {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

// ============================================================================
// drop_replication_profile
// ============================================================================

/// Drop a named replication profile.
pub fn drop_replication_profile(name: impl Into<String>) -> BatchOp {
    BatchOp::DropReplicationProfile(DropReplicationProfileOp {
        drop_replication_profile: name.into(),
    })
}

// ============================================================================
// create_publication
// ============================================================================

/// Declare a publication that downstream subscribers may pull.
pub fn publication(name: impl Into<String>) -> PublicationBuilder {
    PublicationBuilder {
        name: name.into(),
        scopes: Vec::new(),
    }
}

/// Builder for [`CreatePublicationOp`].
pub struct PublicationBuilder {
    name: String,
    scopes: Vec<ReplScope>,
}

impl PublicationBuilder {
    /// Add a single scope to the publication.
    pub fn scope(mut self, scope: ReplScope) -> Self {
        self.scopes.push(scope);
        self
    }

    /// Add many scopes at once (replaces previously added scopes).
    pub fn scopes(mut self, scopes: impl IntoIterator<Item = ReplScope>) -> Self {
        self.scopes = scopes.into_iter().collect();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreatePublication(CreatePublicationOp {
            create_publication: self.name,
            scopes: self.scopes,
        })
    }
}

impl From<PublicationBuilder> for BatchOp {
    fn from(b: PublicationBuilder) -> Self {
        b.build()
    }
}

impl IntoBatchOp for PublicationBuilder {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

// ============================================================================
// drop_publication
// ============================================================================

/// Drop a publication by name.
pub fn drop_publication(name: impl Into<String>) -> BatchOp {
    BatchOp::DropPublication(DropPublicationOp {
        drop_publication: name.into(),
    })
}

// ============================================================================
// create_subscription
// ============================================================================

/// Subscribe this node to a remote publication bound to a local profile.
///
/// All four arguments are required by the wire DTO; passing them positionally
/// keeps the call self-documenting and avoids a silent empty-string default.
pub fn subscription(
    name: impl Into<String>,
    upstream: impl Into<String>,
    publication: impl Into<String>,
    profile: impl Into<String>,
) -> BatchOp {
    BatchOp::CreateSubscription(CreateSubscriptionOp {
        create_subscription: name.into(),
        upstream: upstream.into(),
        publication: publication.into(),
        profile: profile.into(),
    })
}

// ============================================================================
// drop_subscription
// ============================================================================

/// Drop a subscription by name.
pub fn drop_subscription(name: impl Into<String>) -> BatchOp {
    BatchOp::DropSubscription(DropSubscriptionOp {
        drop_subscription: name.into(),
    })
}

// ============================================================================
// alter_subscription
// ============================================================================

/// Begin an `ALTER SUBSCRIPTION` on an existing subscription.
///
/// Exactly one terminal (`.pause()` / `.resume()` / `.set_profile()`) must be
/// called before `.build()`; `.build()` without a terminal call panics (the op
/// DTO requires a non-default `SubAction`).
pub fn alter_subscription(name: impl Into<String>) -> AlterSubscriptionBuilder {
    AlterSubscriptionBuilder {
        name: name.into(),
        action: None,
    }
}

/// Builder for [`AlterSubscriptionOp`].
pub struct AlterSubscriptionBuilder {
    name: String,
    action: Option<SubAction>,
}

impl AlterSubscriptionBuilder {
    /// Pause the subscription.
    pub fn pause(mut self) -> Self {
        self.action = Some(SubAction::Pause);
        self
    }

    /// Resume a paused subscription.
    pub fn resume(mut self) -> Self {
        self.action = Some(SubAction::Resume);
        self
    }

    /// Rebind the subscription to a different replication profile.
    pub fn set_profile(mut self, profile: impl Into<String>) -> Self {
        self.action = Some(SubAction::SetProfile(profile.into()));
        self
    }

    /// Finalize into a [`BatchOp`]. Panics if no terminal action was set.
    pub fn build(self) -> BatchOp {
        let action = self.action.expect(
            "alter_subscription().build() requires a terminal action (pause/resume/set_profile)",
        );
        BatchOp::AlterSubscription(AlterSubscriptionOp {
            alter_subscription: self.name,
            action,
        })
    }
}

impl From<AlterSubscriptionBuilder> for BatchOp {
    fn from(b: AlterSubscriptionBuilder) -> Self {
        b.build()
    }
}

impl IntoBatchOp for AlterSubscriptionBuilder {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

// ============================================================================
// read-only introspection ops
// ============================================================================

/// List all publications defined on this node (read-only).
pub fn list_publications() -> BatchOp {
    BatchOp::ListPublications(ListPublicationsOp {
        list_publications: true,
    })
}

/// List all subscriptions defined on this node (read-only).
pub fn list_subscriptions() -> BatchOp {
    BatchOp::ListSubscriptions(ListSubscriptionsOp {
        list_subscriptions: true,
    })
}

/// Inspect the runtime replication status of this node (read-only).
pub fn replication_status() -> BatchOp {
    BatchOp::ReplicationStatus(ReplicationStatusOp {
        replication_status: true,
    })
}
