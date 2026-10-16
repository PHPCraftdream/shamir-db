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
    DropPublicationOp, DropReplicationProfileOp, DropSubscriptionOp, ReplStream, SubAction,
};
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::repl_binding::{
    quarantined_row_keys, row_is_quarantined, SubscriptionBinding, SUBSCRIPTION_BINDING_VERSION,
};
use crate::types::value::QueryValue;
use crate::{DbResult, ShamirDb};
use shamir_query_types::admin::ReplDirection;
use shamir_query_types::ReplicationServe;
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, db_error_to_batch, to_qv};

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
        let _control = self.shamir.replication_control_lock.lock().await;

        let name = op.create_replication_profile.clone();
        self.shamir
            .validate_seed_profile_change(&name, &op.streams)
            .await
            .map_err(super::replication_seed::control::batch_error)?;

        // S5c-X PR8b-2: an UPSERT must not change the bound destination set
        // of referencing subscription rows (`validate_seed_profile_change`
        // above keeps running for the receipt-constrained rows).
        let profile_table = self
            .shamir
            .system_store()
            .replication_profiles_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let profile_exists = Self::read_all(&profile_table, "replication_profiles")
            .await
            .map_err(|e| err(e.to_string()))?
            .iter()
            .any(|row| row.get("name").and_then(|v| v.as_str()) == Some(name.as_str()));
        if profile_exists {
            self.shamir
                .ensure_profile_binding_unchanged(&name, &op.streams)
                .await
                .map_err(|e| {
                    BatchError::query_coded("", "binding_destination_mismatch", e.to_string())
                })?;
        }

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
        self.shamir
            .system_store()
            .persist_table_interner(&table, None)
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
        let _control = self.shamir.replication_control_lock.lock().await;

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
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        self.authorize_repl_mutation().await?;
        let _control = self.shamir.replication_control_lock.lock().await;

        let name = op.create_publication.clone();

        // S5c-X 2b: an ephemeral backend (no persistent WAL) may only be
        // published downstream when the repo's durability policy sets an
        // EXPLICIT `replication_serve = completed_reseed` — the operator has
        // acknowledged that every data-losing reopen mints a new incarnation
        // and downstream followers must reseed. Checked BEFORE any store
        // write: a rejected publication leaves the catalogue untouched.
        for scope in &op.scopes {
            let Some(db) = self.shamir.get_db(&scope.db) else {
                // Unknown db: nothing to check (no repos resolve).
                continue;
            };
            let repos: Vec<String> = match &scope.repo {
                Some(repo) => vec![repo.clone()],
                None => db.list_repos(),
            };
            for repo_name in repos {
                let Some(repo) = db.get_repo(&repo_name) else {
                    continue;
                };
                if repo.has_persistent_wal() {
                    continue;
                }
                if repo.durability_policy().replication_serve() != ReplicationServe::CompletedReseed
                {
                    return Err(err_code(
                        "ephemeral_backend_requires_completed_reseed",
                        format!(
                            "repository '{}/{repo_name}' runs on an ephemeral backend (no \
                             persistent WAL); publishing it requires an explicit durability \
                             override `replication_serve: completed_reseed`",
                            scope.db
                        ),
                    ));
                }
            }
        }

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
        self.shamir
            .system_store()
            .persist_table_interner(&table, None)
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
        let _control = self.shamir.replication_control_lock.lock().await;

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
        let _control = self.shamir.replication_control_lock.lock().await;

        let name = op.create_subscription.clone();
        let table = self
            .shamir
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| err(e.to_string()))?;

        // S5c-X PR8b-2 (a): refuse same-name overwrite (binding must never
        // detach coverage from data already written).
        self.shamir
            .ensure_subscription_absent(&name)
            .await
            .map_err(|_| {
                BatchError::query_coded(
                    "",
                    "already_exists",
                    format!(
                        "subscription '{name}' already exists: same-name overwrite is refused (S5c-X PR8b-2); use a new name"
                    ),
                )
            })?;

        let mut m = crate::types::common::new_map();
        m.insert("name".to_string(), QueryValue::Str(name.clone()));
        m.insert("upstream".to_string(), QueryValue::Str(op.upstream.clone()));
        m.insert(
            "publication".to_string(),
            QueryValue::Str(op.publication.clone()),
        );
        m.insert("profile".to_string(), QueryValue::Str(op.profile.clone()));
        // `state` drives 386-b pause/resume; a fresh subscription is active.
        m.insert(
            "state".to_string(),
            QueryValue::Str(if op.paused { "paused" } else { "active" }.to_string()),
        );
        let mut record = QueryValue::Map(m);
        self.shamir
            .bind_seed_creation(op, &mut record)
            .await
            .map_err(super::replication_seed::control::batch_error)?;

        // S5c-X PR8b-2 (b): the profile must resolve NOW (a refusal
        // otherwise) and the concrete Pull destinations become the row's
        // durable binding, in the SAME subscription-row transaction.
        let streams = super::replication_seed::catalogue::streams(self, &op.profile)
            .await
            .map_err(super::replication_seed::control::batch_error)?;
        let destinations =
            SubscriptionBinding::from_streams(&name, &streams).map_err(db_error_to_batch)?;
        let binding_row = SubscriptionBinding::new(SUBSCRIPTION_BINDING_VERSION, 1, destinations)
            .map_err(db_error_to_batch)?;
        binding_row.write_to_row(&mut record);

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
        self.shamir
            .system_store()
            .persist_table_interner(&table, None)
            .await
            .map_err(|e| err(e.to_string()))?;
        self.shamir
            .flush_seed_catalogue()
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
        let _control = self.shamir.replication_control_lock.lock().await;

        let name = op.drop_subscription.clone();
        let table = self
            .shamir
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| err(e.to_string()))?;

        // S5c-X PR8b-2: dropping a quarantined subscription is refused —
        // dropping is not evidence of consistency. BEFORE the delete.
        self.shamir
            .ensure_drop_allowed(&name)
            .await
            .map_err(db_error_to_batch)?;

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
        if result {
            // PR8b: the delete can remove a non-quarantined row's cache
            // entry. Boxed (Round-14): await state off the caller's frame.
            Box::pin(self.shamir.refresh_resync_quarantine_cache())
                .await
                .map_err(|e| err(e.to_string()))?;
        }
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
        let _control = self.shamir.replication_control_lock.lock().await;

        let name = op.alter_subscription.clone();
        let table = self
            .shamir
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| err(e.to_string()))?;
        let refs = crate::types::common::new_map();
        let ctx = table
            .info_store_read_view()
            .filter_context(&refs)
            .await
            .map_err(|e| err(e.to_string()))?;
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
        // S5c-X PR8b: any action on a quarantined (`resync_required` or
        // durably `quarantined`-marked) subscription is refused until a
        // replacement seed has been bound since the quarantine began —
        // compared via the durable `resync_from_receipt` baseline
        // `mark_subscription_terminal` wrote against the row's current
        // `seed_receipt`. Pause and SetProfile on a quarantined row are
        // refused too: a state flip can never clear the quarantine (operators
        // must reseed or DROP). This gate runs FIRST: no row mutation (and no
        // `seed_resume_validated` flip) happens on refusal, and the typed
        // error names the runbook.
        if row_is_quarantined(&record) {
            let reseeded = match &op.action {
                SubAction::Resume => {
                    let baseline = record.get("resync_from_receipt");
                    let bound = record.get("seed_receipt");
                    matches!((baseline, bound), (Some(from), Some(now)) if from != now)
                }
                // Pause / SetProfile: refused outright on a quarantined row.
                _ => false,
            };
            if !reseeded {
                return Err(BatchError::query_coded(
                    "",
                    "resync_required",
                    format!(
                        "subscription '{name}' is quarantined (resync_required): \
                             install a replacement seed per guide/09-seed-recovery.md, then Resume"
                    ),
                ));
            }
        }
        self.shamir
            .validate_seed_resume_action(&record, &op.action)
            .await
            .map_err(super::replication_seed::control::batch_error)?;
        // S5c-X PR8b-2: SetProfile must not change the destination set —
        // resolve the proposed profile (a missing profile is a typed
        // refusal) and require exact equality with the row's binding.
        if let SubAction::SetProfile(proposed_profile) = &op.action {
            let streams = super::replication_seed::catalogue::streams(self, proposed_profile)
                .await
                .map_err(super::replication_seed::control::batch_error)?;
            self.shamir
                .ensure_set_profile_binding_unchanged(&record, proposed_profile, &streams)
                .await
                .map_err(|e| {
                    BatchError::query_coded("", "binding_destination_mismatch", e.to_string())
                })?;
        }
        if let QueryValue::Map(ref mut m) = record {
            match &op.action {
                SubAction::Pause => {
                    m.insert("state".to_string(), QueryValue::Str("paused".to_string()));
                }
                SubAction::Resume => {
                    // Refuse to reactivate over a dead destination gate. A
                    // missing profile or destination is not this check's concern.
                    let profile = m
                        .get("profile")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned);
                    if let Some(profile) = profile {
                        let profiles = self
                            .shamir
                            .system_store()
                            .replication_profiles_table()
                            .await
                            .map_err(|error| err(error.to_string()))?;
                        let rows = Self::read_all(&profiles, "replication_profiles")
                            .await
                            .map_err(|error| err(error.to_string()))?;
                        let streams = rows
                            .iter()
                            .find(|row| {
                                row.get("name").and_then(|value| value.as_str())
                                    == Some(profile.as_str())
                            })
                            .and_then(|row| row.get("streams"));
                        if let Some(streams) = streams {
                            let bytes = rmp_serde::to_vec_named(streams)
                                .map_err(|error| err(error.to_string()))?;
                            let streams: Vec<ReplStream> = rmp_serde::from_slice(&bytes)
                                .map_err(|error| err(error.to_string()))?;
                            for stream in streams {
                                if !matches!(stream.direction, ReplDirection::Pull) {
                                    continue;
                                }
                                let Some(repo) = stream.scope.repo else {
                                    continue;
                                };
                                let Some(destination) = self
                                    .shamir
                                    .get_db(&stream.scope.db)
                                    .and_then(|db| db.get_repo(&repo))
                                else {
                                    continue;
                                };
                                let gate = destination
                                    .tx_gate()
                                    .await
                                    .map_err(|error| err(error.to_string()))?;
                                if gate.requires_reopen() {
                                    return Err(err_code(
                                        "reopen_required",
                                        format!(
                                            "Destination {}/{repo} requires reopen",
                                            stream.scope.db
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                    // S5c-X PR8b: a successful (post-reseed) Resume lifts the
                    // quarantine — remove the durable marker and its receipt
                    // baseline alongside the state flip.
                    m.shift_remove("status_message");
                    m.shift_remove("quarantined");
                    m.shift_remove("resync_from_receipt");
                    m.insert("seed_resume_validated".into(), QueryValue::Bool(true));
                    m.insert("state".to_string(), QueryValue::Str("active".to_string()));
                }
                SubAction::SetProfile(p) => {
                    // Binding and generation stay unchanged: the destination
                    // set was already proven identical above.
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
        self.shamir
            .system_store()
            .persist_table_interner(&table, None)
            .await
            .map_err(|e| err(e.to_string()))?;
        // Round-12 (stack layout): pinned like the refresh below.
        Box::pin(self.shamir.flush_seed_catalogue())
            .await
            .map_err(|e| err(e.to_string()))?;
        // PR8b: Pause/Resume/SetProfile can flip the persisted `state`; rebuild
        // the read-quarantine cache post-commit. Boxed (Round-14): await state
        // off the caller's frame.
        Box::pin(self.shamir.refresh_resync_quarantine_cache())
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
        let entries = self
            .shamir
            .seed_status_entries(records)
            .await
            .map_err(|e| err(e.to_string()))?;
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
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
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
                    let refs = crate::types::common::new_map();
                    let ctx = owned_table
                        .info_store_read_view()
                        .filter_context(&refs)
                        .await?;
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
// NOT a `ShamirAdminExecutor`. So this helper is exposed directly on
// `ShamirDb`, reusing the same lookup + `set_via_implicit_tx` write pattern
// `handle_alter_subscription`'s `Pause`/`Resume` arms use above,
// arms use above — just without the actor-authorization gate, since this is
// an internal system action taken by the replication engine itself (mirrors
// `Actor::System` already used by `delete_repl`/`SystemStore::set_via_implicit_tx`
// throughout this file), not a user-initiated admin command.
impl ShamirDb {
    /// Does the quarantine cover `(db_name, repo_name)`? Constant time: one
    /// snapshot load + one exact `(db, repo)` lookup in the lock-free cache
    /// — no catalogue read, no profile resolution, no wildcard scope (rows
    /// bind concrete Pull destinations only). Called by the replication
    /// serve path (S5c-X 2e).
    pub async fn has_resync_required_subscription_covering(
        &self,
        db_name: &str,
        repo_name: &str,
    ) -> DbResult<bool> {
        Ok(self.quarantined_read_reason(db_name, repo_name).is_some())
    }
    /// Quarantine reason for an ordinary READ of `(db_name, repo_name)`, or
    /// `None` when the repo is readable. Healthy path (empty cache): one
    /// `Relaxed` `is_empty` load and no allocation / map lock — this is what
    /// the wire-layer cursor gate in `shamir-server` rides on.
    pub(crate) fn quarantined_read_reason(&self, db_name: &str, repo_name: &str) -> Option<String> {
        // Healthy-path fast path: one Relaxed load, no allocation, no map
        // lock when the cache is empty (no subscription is quarantined).
        if self.resync_quarantine.is_empty() {
            return None;
        }
        self.resync_quarantine.reason_for(db_name, repo_name)
    }

    /// Public seam for wire-layer read gates (e.g. cursor-based journal reads
    /// in the server crate): the read-quarantine (`resync_required`) reason
    /// for `(db_name, repo_name)`, or `None` when the repo is readable.
    /// Mirrors the internal gate of
    /// [`read_changelog_bounded`](crate::ShamirDb::read_changelog_bounded).
    pub fn read_quarantine_reason(&self, db_name: &str, repo_name: &str) -> Option<String> {
        self.quarantined_read_reason(db_name, repo_name)
    }

    /// Persist `state = "resync_required"` on subscription `name`'s row in
    /// `system/subscriptions`.
    ///
    /// Called by the follower-loop supervisor (`shamir-server`) on a
    /// terminal journal-gap error: the
    /// follower is permanently missing `[from_version, gap_at)` and must
    /// stop, and this makes that stop visible via the existing
    /// `ReplicationStatus`/`ListSubscriptions` admin surface (both already
    /// echo `state` verbatim). Because `reconcile()` only (re)starts
    /// subscriptions whose row has `state == "active"`, a row left at
    /// `"resync_required"` naturally stays stopped across reconcile ticks,
    /// exactly like `"paused"` does today. Recovery requires a replacement seed
    /// install (which binds a NEW `seed_receipt` on the row, differing from
    /// the `resync_from_receipt` baseline captured here) followed by the
    /// `Resume` admin action — a plain `Resume` of a still-quarantined row
    /// is refused with a typed error. The quarantine is additionally marked
    /// with a durable `quarantined` flag on the row, which — unlike `state`
    /// — survives Pause/SetProfile: any admin action on a quarantined row
    /// except a post-reseed `Resume` is refused (S5c-X PR8b-2 adds DROP to
    /// the refusals).
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
        self.mark_subscription_terminal(
            name,
            "resync_required",
            &format!("journal gap at {gap_at}, requested from {from_version}"),
        )
        .await
    }

    /// Supervisor seam (stage W2): like
    /// [`Self::mark_subscription_resync_required`] but generation-aware — a
    /// stale callback whose generation no longer matches is a logged no-op.
    pub async fn mark_subscription_resync_required_generation(
        &self,
        name: &str,
        gap_at: u64,
        from_version: u64,
        expected_binding_generation: u64,
    ) -> DbResult<()> {
        self.mark_subscription_terminal_generation(
            name,
            "resync_required",
            &format!("journal gap at {gap_at}, requested from {from_version}"),
            Some(expected_binding_generation),
        )
        .await
    }

    /// Persist an operational stop reason exposed verbatim by status/list.
    /// A `"resync_required"` state additionally stamps the durable
    /// `quarantined` marker plus the `resync_from_receipt` baseline;
    /// coverage comes from the row's durable binding.
    pub async fn mark_subscription_terminal(
        &self,
        name: &str,
        state: &str,
        message: &str,
    ) -> DbResult<()> {
        self.mark_subscription_terminal_generation(name, state, message, None)
            .await
    }

    /// Generation-aware terminal marking (S5c-X PR8b-2). When
    /// `expected_binding_generation` is given and the row's
    /// `binding_generation` differs, the callback is stale (it raced a
    /// replacement binding) — log a warning and return `Ok(())` WITHOUT
    /// touching the row or the cache.
    pub async fn mark_subscription_terminal_generation(
        &self,
        name: &str,
        state: &str,
        message: &str,
        expected_binding_generation: Option<u64>,
    ) -> DbResult<()> {
        let _control = self.replication_control_lock.lock().await;
        let mut next_entries = self.resync_quarantine_entries().await?;
        let table = self.system_store().subscriptions_table().await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
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
                 (state={state}); treating as a no-op \
                 — it may have been dropped concurrently"
            );
            return Ok(());
        };
        let mut record = row.as_value().into_owned();
        if let Some(expected) = expected_binding_generation {
            let row_generation = record.get("binding_generation").and_then(|v| v.as_u64());
            if row_generation != Some(expected) {
                log::warn!(
                    "mark_subscription_terminal: subscription '{name}' binding generation \
                     {row_generation:?} != expected {expected}; ignoring stale callback \
                     (a replacement binding must never be marked or cleared by an old task)"
                );
                return Ok(());
            }
        }
        if state == "resync_required" {
            // S5c-X PR8b-2: coverage is the row's durable binding — an
            // unparseable row fails loudly (typed error), never marks
            // nothing. Ordering preserved: (1) publish, (2) revoke+drain,
            // (3) persist+interner+flush, (4) refresh. On persist/flush
            // failure the prepublished keys stay (refresh is not reached).
            let binding = SubscriptionBinding::from_row(&record)?;
            next_entries.extend(quarantined_row_keys(&record, &binding, message));
            self.resync_quarantine.publish(next_entries);
            Box::pin(self.revoke_quarantined_receivers()).await; // Round-14 boxed
        }
        // S5c-X PR8b: capture the row's CURRENT `seed_receipt` as the durable
        // baseline the quarantine began from (absent ⇒ `Null`). `Resume`'s
        // gate requires the bound receipt to differ from this value, so a
        // plain `Resume` can never lift the quarantine without a replacement
        // seed installed in between.
        let resync_from_receipt = record
            .get("seed_receipt")
            .cloned()
            .unwrap_or(QueryValue::Null);
        if let QueryValue::Map(ref mut m) = record {
            m.insert("state".to_string(), QueryValue::Str(state.to_string()));
            m.insert("resync_from_receipt".into(), resync_from_receipt);
            if state == "resync_required" {
                // Durable quarantine marker: unlike `state`, it is not
                // cleared by Pause/SetProfile — only a successful post-reseed
                // `Resume` removes it.
                m.insert("quarantined".into(), QueryValue::Bool(true));
            }
            m.insert(
                "last_terminal_reason".into(),
                QueryValue::Str(state.to_string()),
            );
            m.insert(
                "status_message".into(),
                QueryValue::Str(message.to_string()),
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
        self.system_store()
            .persist_table_interner(&table, None)
            .await?;
        self.flush_seed_catalogue().await?;
        Box::pin(self.refresh_resync_quarantine_cache()).await?; // Round-14 boxed
        Ok(())
    }
}
