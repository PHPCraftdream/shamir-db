//! Admin handlers: replication DDL (386-a).
//!
//! Persistence + CRUD + read for the publication/subscription model. These
//! handlers write `replication_profiles` / `publications` / `subscriptions`
//! records into the `system` repo via [`SystemStore`], mirroring the
//! users/roles model (`admin_users_roles.rs`): so the catalogue itself
//! replicates (V1a). Starting the follower pull-loop from a subscription is a
//! *separate* concern (386-b) and is intentionally NOT done here — these ops
//! only make the definitions durable and queryable.
//!
//! Authorization: every mutating op is global-admin only (`Manage` on
//! [`ResourcePath::Root`]) — the closest analog is `handle_create_role`.
//! Read-only introspection (`list_*`, `replication_status`) requires `List`
//! on the root, matching `ListOp::Databases`.

use crate::access::{Action, ResourcePath};
use crate::query::admin::{
    AlterSubscriptionOp, CreatePublicationOp, CreateReplicationProfileOp, CreateSubscriptionOp,
    DropPublicationOp, DropReplicationProfileOp, DropSubscriptionOp, SubAction,
};
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::types::value::QueryValue;
use crate::{DbResult, ShamirDb};
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, to_qv};

impl ShamirAdminExecutor {
    // ------------------------------------------------------------------
    // shared guards / small helpers
    // ------------------------------------------------------------------

    /// Authorize a replication mutation: global-admin only (Manage on the
    /// root). System bypasses inside `authorize_access`.
    async fn authorize_repl_mutation(&self) -> Result<(), BatchError> {
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await
            .map_err(|e| BatchError::QueryError {
                alias: String::new(),
                message: e.to_string(),
                code: Some("access_denied".to_string()),
            })
    }

    /// Authorize a replication read (List on the root).
    async fn authorize_repl_read(&self) -> Result<(), BatchError> {
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::List)
            .await
            .map_err(|e| BatchError::QueryError {
                alias: String::new(),
                message: e.to_string(),
                code: Some("access_denied".to_string()),
            })
    }

    // ------------------------------------------------------------------
    // replication profiles
    // ------------------------------------------------------------------

    pub(super) async fn handle_create_replication_profile(
        &self,
        op: &CreateReplicationProfileOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        self.authorize_repl_mutation().await?;

        let name = op.create_replication_profile.clone();
        let mut m = crate::types::common::new_map();
        m.insert("name".to_string(), QueryValue::Str(name.clone()));
        m.insert("streams".to_string(), to_qv(&op.streams));
        let record = QueryValue::Map(m);

        let table = self
            .shamir
            .system_store()
            .replication_profiles_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("replication_profiles"),
            key: mpack!({"name": @(QueryValue::Str(name.clone()))}),
            value: record,
        };
        self.shamir
            .system_store()
            .set_via_implicit_tx(&table, &set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "created_replication_profile": @(QueryValue::Str(name)),
        })))
    }

    pub(super) async fn handle_drop_replication_profile(
        &self,
        op: &DropReplicationProfileOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        self.authorize_repl_mutation().await?;

        let name = op.drop_replication_profile.clone();
        let table = self
            .shamir
            .system_store()
            .replication_profiles_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let del_op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new("replication_profiles"),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.clone()),
            },
            select: None,
            expected_version: None,
        };
        let result = self.delete_repl(&table, &del_op).await?;
        Ok(admin_result(mpack!({
            "dropped_replication_profile": @(QueryValue::Str(name)),
            "existed": @(QueryValue::Bool(result)),
        })))
    }

    // ------------------------------------------------------------------
    // publications
    // ------------------------------------------------------------------

    pub(super) async fn handle_create_publication(
        &self,
        op: &CreatePublicationOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        self.authorize_repl_mutation().await?;

        let name = op.create_publication.clone();
        let mut m = crate::types::common::new_map();
        m.insert("name".to_string(), QueryValue::Str(name.clone()));
        m.insert("scopes".to_string(), to_qv(&op.scopes));
        let record = QueryValue::Map(m);

        let table = self
            .shamir
            .system_store()
            .publications_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("publications"),
            key: mpack!({"name": @(QueryValue::Str(name.clone()))}),
            value: record,
        };
        self.shamir
            .system_store()
            .set_via_implicit_tx(&table, &set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "created_publication": @(QueryValue::Str(name)),
        })))
    }

    pub(super) async fn handle_drop_publication(
        &self,
        op: &DropPublicationOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        self.authorize_repl_mutation().await?;

        let name = op.drop_publication.clone();
        let table = self
            .shamir
            .system_store()
            .publications_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let del_op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new("publications"),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.clone()),
            },
            select: None,
            expected_version: None,
        };
        let result = self.delete_repl(&table, &del_op).await?;
        Ok(admin_result(mpack!({
            "dropped_publication": @(QueryValue::Str(name)),
            "existed": @(QueryValue::Bool(result)),
        })))
    }

    pub(super) async fn handle_list_publications(&self) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        self.authorize_repl_read().await?;

        let table = self
            .shamir
            .system_store()
            .publications_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let records = Self::read_all(&table, "publications")
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "publications": @(QueryValue::List(records)),
        })))
    }

    // ------------------------------------------------------------------
    // subscriptions
    // ------------------------------------------------------------------

    pub(super) async fn handle_create_subscription(
        &self,
        op: &CreateSubscriptionOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        self.authorize_repl_mutation().await?;

        let name = op.create_subscription.clone();
        let mut m = crate::types::common::new_map();
        m.insert("name".to_string(), QueryValue::Str(name.clone()));
        m.insert("upstream".to_string(), QueryValue::Str(op.upstream.clone()));
        m.insert(
            "publication".to_string(),
            QueryValue::Str(op.publication.clone()),
        );
        m.insert("profile".to_string(), QueryValue::Str(op.profile.clone()));
        // `state` drives 386-b pause/resume; a fresh subscription is active.
        m.insert("state".to_string(), QueryValue::Str("active".to_string()));
        let record = QueryValue::Map(m);

        let table = self
            .shamir
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("subscriptions"),
            key: mpack!({"name": @(QueryValue::Str(name.clone()))}),
            value: record,
        };
        self.shamir
            .system_store()
            .set_via_implicit_tx(&table, &set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "created_subscription": @(QueryValue::Str(name)),
        })))
    }

    pub(super) async fn handle_drop_subscription(
        &self,
        op: &DropSubscriptionOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        self.authorize_repl_mutation().await?;

        let name = op.drop_subscription.clone();
        let table = self
            .shamir
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let del_op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new("subscriptions"),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.clone()),
            },
            select: None,
            expected_version: None,
        };
        let result = self.delete_repl(&table, &del_op).await?;
        Ok(admin_result(mpack!({
            "dropped_subscription": @(QueryValue::Str(name)),
            "existed": @(QueryValue::Bool(result)),
        })))
    }

    pub(super) async fn handle_alter_subscription(
        &self,
        op: &AlterSubscriptionOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        self.authorize_repl_mutation().await?;

        let name = op.alter_subscription.clone();
        let table = self
            .shamir
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let interner = table
            .interner()
            .get()
            .await
            .map_err(|e| err(e.to_string()))?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let lookup = crate::query::read::ReadQuery::new("subscriptions").filter(
            crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.clone()),
            },
        );
        let existing = table
            .read(&lookup, &ctx)
            .await
            .map_err(|e| err(e.to_string()))?;
        if existing.records.is_empty() {
            return Err(err_code(
                "not_found",
                format!("Subscription '{}' not found", name),
            ));
        }

        let mut record = existing.records[0].as_value().into_owned();
        if let QueryValue::Map(ref mut m) = record {
            match &op.action {
                SubAction::Pause => {
                    m.insert("state".to_string(), QueryValue::Str("paused".to_string()));
                }
                SubAction::Resume => {
                    m.insert("state".to_string(), QueryValue::Str("active".to_string()));
                }
                SubAction::SetProfile(p) => {
                    m.insert("profile".to_string(), QueryValue::Str(p.clone()));
                }
            }
        }
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("subscriptions"),
            key: mpack!({"name": @(QueryValue::Str(name.clone()))}),
            value: record,
        };
        self.shamir
            .system_store()
            .set_via_implicit_tx(&table, &set_op)
            .await
            .map_err(|e| err(e.to_string()))?;
        table
            .interner()
            .persist()
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "altered_subscription": @(QueryValue::Str(name)),
        })))
    }

    pub(super) async fn handle_list_subscriptions(&self) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        self.authorize_repl_read().await?;

        let table = self
            .shamir
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let records = Self::read_all(&table, "subscriptions")
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "subscriptions": @(QueryValue::List(records)),
        })))
    }

    pub(super) async fn handle_replication_status(&self) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        self.authorize_repl_read().await?;

        // 386-a: status is the set of subscriptions with their `state`.
        // Runtime lag / applied-LSN is 386-b.
        let table = self
            .shamir
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let records = Self::read_all(&table, "subscriptions")
            .await
            .map_err(|e| err(e.to_string()))?;
        let entries: Vec<QueryValue> = records
            .into_iter()
            .map(|rec| {
                let name = rec.get("name").cloned().unwrap_or(QueryValue::Null);
                let state = rec.get("state").cloned().unwrap_or(QueryValue::Null);
                let mut m = crate::types::common::new_map();
                m.insert("name".to_string(), name);
                m.insert("state".to_string(), state);
                QueryValue::Map(m)
            })
            .collect();
        Ok(admin_result(mpack!({
            "subscriptions": @(QueryValue::List(entries)),
        })))
    }

    // ------------------------------------------------------------------
    // internal storage helpers
    // ------------------------------------------------------------------

    /// Read every record of a system-store table as owned `QueryValue`s.
    async fn read_all(
        table: &crate::engine::table::TableManager,
        table_name: &str,
    ) -> crate::DbResult<Vec<QueryValue>> {
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(table_name);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| r.as_value().into_owned())
            .collect())
    }

    /// Route a repl delete through the implicit-tx file-WAL path. Returns
    /// whether any record was removed.
    async fn delete_repl(
        &self,
        table: &crate::engine::table::TableManager,
        op: &crate::query::write::DeleteOp,
    ) -> Result<bool, BatchError> {
        use crate::access::Actor;
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let repo = self
            .shamir
            .system_store()
            .system_repo()
            .map_err(|e| err(e.to_string()))?;
        let owned_op = op.clone();
        let owned_table = table.clone();
        let result = repo
            .run_implicit_batch_tx(Actor::System, "", move |tx| {
                Box::pin(async move {
                    let interner = owned_table.interner().get().await?;
                    let refs = crate::types::common::new_map();
                    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
                    owned_table
                        .execute_delete_tx(&owned_op, &ctx, tx, None, &Actor::System)
                        .await
                })
            })
            .await?;
        Ok(result.affected > 0)
    }
}

// ----------------------------------------------------------------------
// Follower-side gap visibility (Part 2, RI-10)
// ----------------------------------------------------------------------
//
// `SubscriptionSupervisor` (`shamir-server`) only holds an `Arc<ShamirDb>` —
// NOT a `ShamirAdminExecutor` (which additionally requires a resolved
// `Actor` + `db_name` for a specific caller session, neither of which the
// supervisor's background follower-loop task has). `ShamirAdminExecutor`
// itself is `pub(super)` to this crate's `shamir_db` module tree and
// unreachable from `shamir-server` regardless. So this helper is exposed
// directly on `ShamirDb`, reusing the exact same lookup + `set_via_implicit_tx`
// write pattern `handle_alter_subscription`'s `SubAction::Pause`/`Resume`
// arms use above — just without the actor-authorization gate, since this is
// an internal system action taken by the replication engine itself (mirrors
// `Actor::System` already used by `delete_repl`/`SystemStore::set_via_implicit_tx`
// throughout this file), not a user-initiated admin command.
impl ShamirDb {
    /// Persist `state = "resync_required"` on subscription `name`'s row in
    /// `system/subscriptions`.
    ///
    /// Called by the follower-loop supervisor (`shamir-server`) when
    /// [`run_follower_loop`](../../../shamir-server/src/replication/follower_loop.rs)
    /// (out of this crate) returns a terminal journal-gap error: the
    /// follower is permanently missing `[from_version, gap_at)` and must
    /// stop, and this makes that stop visible via the existing
    /// `ReplicationStatus`/`ListSubscriptions` admin surface (both already
    /// echo `state` verbatim). Because `reconcile()` only (re)starts
    /// subscriptions whose row has `state == "active"`, a row left at
    /// `"resync_required"` naturally stays stopped across reconcile ticks,
    /// exactly like `"paused"` does today. Recovery is the EXISTING
    /// `Resume` admin action (flips `state` back to `"active"`); no new
    /// admin action is introduced.
    ///
    /// `name` not found → treated as a no-op (logged, not errored): the
    /// subscription may have been dropped concurrently by an admin command
    /// racing with the gap detection.
    pub async fn mark_subscription_resync_required(
        &self,
        name: &str,
        gap_at: u64,
        from_version: u64,
    ) -> DbResult<()> {
        let table = self.system_store().subscriptions_table().await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let lookup = crate::query::read::ReadQuery::new("subscriptions").filter(
            crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
        );
        let existing = table.read(&lookup, &ctx).await?;
        let Some(row) = existing.records.into_iter().next() else {
            log::warn!(
                "mark_subscription_resync_required: subscription '{name}' not found \
                 (gap_at={gap_at}, from_version={from_version}); treating as a no-op \
                 — it may have been dropped concurrently"
            );
            return Ok(());
        };

        let mut record = row.as_value().into_owned();
        if let QueryValue::Map(ref mut m) = record {
            m.insert(
                "state".to_string(),
                QueryValue::Str("resync_required".to_string()),
            );
        }
        let set_op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("subscriptions"),
            key: mpack!({"name": @(QueryValue::Str(name.to_string()))}),
            value: record,
        };
        self.system_store()
            .set_via_implicit_tx(&table, &set_op)
            .await?;
        table.interner().persist().await?;
        Ok(())
    }
}
