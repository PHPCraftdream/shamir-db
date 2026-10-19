//! Admin handlers: replication DDL (386-a). Rows go into the `system` repo
//! via [`SystemStore`] so the catalogue itself replicates (V1a); the
//! follower pull-loop is 386-b. Mutating ops are global-admin only
//! (`Manage` on [`ResourcePath::Root`]); introspection requires `List`.

use crate::access::{Action, ResourcePath};
use crate::query::admin::{
    AlterSubscriptionOp, CreatePublicationOp, CreateReplicationProfileOp, CreateSubscriptionOp,
    DropPublicationOp, DropReplicationProfileOp, DropSubscriptionOp, ReplStream, SubAction,
};
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::repl_binding::pending::PENDING_STATE;
use crate::repl_binding::{
    quarantined_row_keys, row_is_quarantined, SubscriptionBinding, SUBSCRIPTION_BINDING_VERSION,
};
use crate::types::value::QueryValue;
use crate::{DbError, DbResult, ShamirDb};
use futures::StreamExt;
use shamir_query_types::admin::ReplDirection;
use shamir_query_types::ReplicationServe;
use shamir_types::access::Actor;
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, db_error_to_batch, repl_guard_error_to_batch, to_qv};

/// The uncoded [`BatchError`] mapper shared by this file's handlers.
pub(super) fn batch_err(msg: String) -> BatchError {
    BatchError::QueryError {
        alias: String::new(),
        message: msg,
        code: None,
    }
}

impl ShamirAdminExecutor {
    /// Authorize a replication mutation: global-admin only (Manage on root).
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

    /// Authorize a replication read (List on root).
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

    // ---------------------- replication profiles ----------------------

    pub(super) async fn handle_create_replication_profile(
        &self,
        op: &CreateReplicationProfileOp,
    ) -> Result<QueryResult, BatchError> {
        let err = batch_err;
        self.authorize_repl_mutation().await?;
        let _control = self.shamir.lock_replication_control().await;

        let name = op.create_replication_profile.clone();
        self.shamir
            .validate_seed_profile_change(&name, &op.streams)
            .await
            .map_err(super::replication_seed::control::batch_error)?;

        // S5c-X PR8b-2: an UPSERT must not change the bound destination set
        // of referencing subscription rows.
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
        let err = batch_err;
        self.authorize_repl_mutation().await?;
        let _control = self.shamir.lock_replication_control().await;

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

    // ------------------------- publications ---------------------------

    pub(super) async fn handle_create_publication(
        &self,
        op: &CreatePublicationOp,
    ) -> Result<QueryResult, BatchError> {
        let err = batch_err;
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        self.authorize_repl_mutation().await?;
        let _control = self.shamir.lock_replication_control().await;

        let name = op.create_publication.clone();

        // S5c-X 2b: an ephemeral backend (no persistent WAL) may only be
        // published when the durability policy sets an EXPLICIT
        // `replication_serve = completed_reseed`. Checked BEFORE any write.
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
        let err = batch_err;
        self.authorize_repl_mutation().await?;
        let _control = self.shamir.lock_replication_control().await;

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
        let err = batch_err;
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

    // ------------------------ subscriptions ---------------------------

    pub(super) async fn handle_create_subscription(
        &self,
        op: &CreateSubscriptionOp,
    ) -> Result<QueryResult, BatchError> {
        self.authorize_repl_mutation().await?;
        let _control = self.shamir.lock_replication_control().await;

        let name = op.create_subscription.clone();

        // S5c-X PR8b-2 (a): refuse same-name overwrite (binding must never
        // detach coverage from written data).
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
        let mut record = QueryValue::Map(m);
        self.shamir
            .bind_seed_creation(op, &mut record)
            .await
            .map_err(super::replication_seed::control::batch_error)?;

        // S5c-X PR8b-2 (b): the profile must resolve NOW and the concrete
        // Pull destinations become the row's durable binding, in the SAME
        // subscription-row transaction.
        let streams = super::replication_seed::catalogue::streams(&self.shamir, &op.profile)
            .await
            .map_err(super::replication_seed::control::batch_error)?;
        let destinations =
            SubscriptionBinding::from_streams(&name, &streams).map_err(db_error_to_batch)?;
        // Generation placeholder: `write_new_subscription` stamps the
        // monotonic `next_generation` over the catalogue rows.
        let binding_row = SubscriptionBinding::new(SUBSCRIPTION_BINDING_VERSION, 1, destinations)
            .map_err(db_error_to_batch)?;
        binding_row.write_to_row(&mut record);
        // Round 2 (A4): durability-before-start — the row is written as
        // `pending_durable` FIRST, THEN flipped; a follower can never
        // observe a startable row whose durable barrier has not flushed.
        let requested_state = if op.paused { "paused" } else { "active" };
        crate::repl_binding::pending::write_new_subscription(&self.shamir, record, requested_state)
            .await
            .map_err(db_error_to_batch)?;
        Ok(admin_result(mpack!({
            "created_subscription": @(QueryValue::Str(name)),
        })))
    }

    pub(super) async fn handle_drop_subscription(
        &self,
        op: &DropSubscriptionOp,
    ) -> Result<QueryResult, BatchError> {
        let err = batch_err;
        self.authorize_repl_mutation().await?;
        let _control = self.shamir.lock_replication_control().await;

        let name = op.drop_subscription.clone();
        let table = self
            .shamir
            .system_store()
            .subscriptions_table()
            .await
            .map_err(|e| err(e.to_string()))?;

        // S5c-X PR8b-2: dropping a quarantined subscription is refused —
        // dropping is not evidence of consistency.
        self.shamir
            .ensure_drop_allowed(&name)
            .await
            .map_err(repl_guard_error_to_batch)?; // quiesce (below) before ANY mutation
        self.shamir
            .quiesce_subscription_lease_locked(&name, &_control)
            .await;

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
            // entry (boxed: Round-14).
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
        let err = batch_err;
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        self.authorize_repl_mutation().await?;
        let _control = self.shamir.lock_replication_control().await;

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
        // S5c-X PR8b-2 round 3 (P1-C): a `pending_durable` row is an
        // unfinished creation barrier with NO durable active state — refuse
        // every action (re-run CREATE or DROP it).
        if record.get("state").and_then(|value| value.as_str()) == Some(PENDING_STATE) {
            return Err(BatchError::query_coded(
                "",
                "pending_creation_barrier",
                format!(
                    "subscription '{name}' is an unfinished creation barrier (pending_durable): \
                     it has no durable active state — re-run CREATE to complete or DROP it"
                ),
            ));
        }
        // S5c-X PR8b: any action on a quarantined subscription is refused
        // until a replacement seed has been bound (durable
        // `resync_from_receipt` baseline); Pause/SetProfile are refused
        // outright. Runs FIRST: no row mutation happens on refusal.
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
                        "subscription '{name}' is quarantined (resync_required): DROP of a quarantined subscription is refused in this version; install a replacement seed per guide/09-seed-recovery.md, then Resume"
                    ),
                ));
            }
        }
        self.shamir
            .validate_seed_resume_action(&record, &op.action)
            .await
            .map_err(super::replication_seed::control::batch_error)?;
        // S5c-X PR8b-2: SetProfile must not change the destination set —
        // resolve the proposed profile and require exact equality with the
        // row's binding.
        if let SubAction::SetProfile(proposed_profile) = &op.action {
            let streams =
                super::replication_seed::catalogue::streams(&self.shamir, proposed_profile)
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
                    // quiesce first: drain an in-flight apply, block new ones
                    self.shamir
                        .quiesce_subscription_lease_locked(&name, &_control)
                        .await;
                    m.insert("state".to_string(), QueryValue::Str("paused".to_string()));
                }
                SubAction::Resume => {
                    // Refuse to reactivate over a dead destination gate.
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
                    // quarantine — remove the marker + baseline fields.
                    m.shift_remove("status_message");
                    m.shift_remove("quarantined");
                    m.shift_remove("resync_from_receipt");
                    m.insert("seed_resume_validated".into(), QueryValue::Bool(true));
                    m.insert("state".to_string(), QueryValue::Str("active".to_string()));
                }
                SubAction::SetProfile(p) => {
                    // Binding/generation unchanged: destinations proven identical above.
                    m.insert("profile".to_string(), QueryValue::Str(p.clone()));
                }
            }
        }
        // Round 7 item 5: destinations whose pending quarantine keys a
        // successful Resume rolls back, under THIS row's `name:generation` owner
        // (an unrelated Resume must never erase another claim); empty Pause/Set.
        let resume_rollback = if matches!(op.action, SubAction::Resume) {
            SubscriptionBinding::from_row(&record).ok().map(|binding| {
                let owner = format!("{}:{}", name, binding.generation());
                let keys = binding.destinations().to_vec();
                let claims = self
                    .shamir
                    .resync_quarantine
                    .owner_claims_for(&owner, &keys);
                (claims, keys)
            })
        } else {
            None
        };
        // Round 6 item 1: Resume `shift_remove`s marker fields, and a plain `SetOp`
        // MERGES - persist via the true-replace helper (one implicit tx staging).
        self.shamir
            .replace_subscription_row(&table, name.clone(), record)
            .await
            .map_err(|e| err(e.to_string()))?;
        self.shamir
            .system_store()
            .persist_table_interner(&table, None)
            .await
            .map_err(|e| err(e.to_string()))?;
        Box::pin(self.shamir.flush_seed_catalogue()) // Round-12 stack pin
            .await
            .map_err(|e| err(e.to_string()))?;
        // PR8b: rebuild the read-quarantine cache post-commit (Round-14).
        Box::pin(self.shamir.refresh_resync_quarantine_cache())
            .await
            .map_err(|e| err(e.to_string()))?;
        if let Some((claims, keys)) = resume_rollback {
            for claim in claims {
                self.shamir
                    .resync_quarantine
                    .rollback_pending(&claim, keys.clone());
            }
        }
        Ok(admin_result(mpack!({
            "altered_subscription": @(QueryValue::Str(name)),
        })))
    }

    pub(super) async fn handle_list_subscriptions(&self) -> Result<QueryResult, BatchError> {
        let err = batch_err;
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
        let err = batch_err;
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

    // -------------------- internal storage helpers --------------------

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

    /// Route a repl delete through the implicit-tx file-WAL path.
    async fn delete_repl(
        &self,
        table: &crate::engine::table::TableManager,
        op: &crate::query::write::DeleteOp,
    ) -> Result<bool, BatchError> {
        use crate::access::Actor;
        let err = batch_err;
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
// NOT a `ShamirAdminExecutor` — so these seams are exposed directly on
// `ShamirDb` as internal system actions, without the authorization gate.
impl ShamirDb {
    /// Does the quarantine cover `(db_name, repo_name)`? Constant time: one
    /// snapshot load + one exact `(db, repo)` lookup — no catalogue/wildcard.
    pub async fn has_resync_required_subscription_covering(
        &self,
        db_name: &str,
        repo_name: &str,
    ) -> DbResult<bool> {
        Ok(self.quarantined_read_reason(db_name, repo_name).is_some())
    }
    /// Quarantine reason for an ordinary READ of `(db_name, repo_name)`, or
    /// `None` when readable (one snapshot load + one map miss; the server
    /// cursor gate rides on this).
    pub(crate) fn quarantined_read_reason(&self, db_name: &str, repo_name: &str) -> Option<String> {
        // One load per lookup: the guard pins the snapshot for the lookup.
        let snapshot = self.resync_quarantine.snapshot();
        snapshot
            .entries
            .get(db_name)?
            .get(repo_name)
            .map(|reason| reason.to_string())
    }

    /// Public seam for wire-layer read gates (server cursor reads): the reason or `None`.
    pub fn read_quarantine_reason(&self, db_name: &str, repo_name: &str) -> Option<String> {
        self.quarantined_read_reason(db_name, repo_name)
    }

    /// Persist `state = "resync_required"` on subscription `name`'s row (supervisor
    /// seam, terminal journal-gap error). Recovery = replacement seed install +
    /// `Resume`; durable `quarantined` survives Pause/SetProfile, other admin
    /// action refused. Missing name → logged no-op.
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
    /// stale callback is a logged no-op.
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

    /// Persist an operational stop reason; `"resync_required"` additionally
    /// stamps the durable `quarantined` marker + receipt baseline.
    pub async fn mark_subscription_terminal(
        &self,
        name: &str,
        state: &str,
        message: &str,
    ) -> DbResult<()> {
        self.mark_subscription_terminal_generation(name, state, message, None)
            .await
    }

    /// Generation-aware terminal marking (S5c-X PR8b-2): a stale callback
    /// (generation mismatch) is a logged no-op WITHOUT touching row/cache.
    pub async fn mark_subscription_terminal_generation(
        &self,
        name: &str,
        state: &str,
        message: &str,
        expected_binding_generation: Option<u64>,
    ) -> DbResult<()> {
        let _control = self.lock_replication_control().await;
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
        self.quiesce_subscription_lease_locked(name, &_control) // quiesce BEFORE the row mutation
            .await;
        let mut published_keys: Vec<(String, String)> = Vec::new();
        let mut published_claims: Vec<String> = Vec::new();
        if state == "resync_required" {
            // S5c-X PR8b-2: coverage is the row's durable binding — an
            // unparseable row fails loudly (typed error). Ordering: (1)
            // publish, (2) revoke+drain, (3) persist+interner+flush, (4)
            // refresh, (5) confirm; any earlier failure leaves the keys
            // pending (round 6 item 5).
            let binding = SubscriptionBinding::from_row(&record)?;
            let logical_owner = format!("{}:{}", name, binding.generation());
            let keys = quarantined_row_keys(&record, &binding, message);
            published_keys = keys.iter().map(|(key, _)| key.clone()).collect();
            self.resync_quarantine.publish(keys, &logical_owner);
            published_claims = self
                .resync_quarantine
                .owner_claims_for(&logical_owner, &published_keys);
            Box::pin(self.revoke_quarantined_receivers()).await; // Round-14 boxed
        }
        // S5c-X PR8b: capture the row's CURRENT `seed_receipt` as the durable
        // baseline the quarantine began from (absent ⇒ `Null`); `Resume`'s
        // gate requires the bound receipt to differ from it, so a plain
        // `Resume` can never lift the quarantine without a replacement seed.
        let resync_from_receipt = record
            .get("seed_receipt")
            .cloned()
            .unwrap_or(QueryValue::Null);
        if let QueryValue::Map(ref mut m) = record {
            m.insert("state".to_string(), QueryValue::Str(state.to_string()));
            m.insert("resync_from_receipt".into(), resync_from_receipt);
            if state == "resync_required" {
                // Durable marker: not cleared by Pause/SetProfile — only a
                // successful post-reseed `Resume` removes it.
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
        // Round 6 item 5 test seam: fail the row SET BEFORE the row
        // changes (pending protection without durability).
        #[cfg(any(test, feature = "test-util"))]
        if self
            .row_set_fault_armed
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(DbError::Storage(
                "injected terminal row SET failure (test)".into(),
            ));
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
        Box::pin(self.refresh_resync_quarantine_cache()).await?;
        for claim in published_claims {
            self.resync_quarantine
                .confirm_pending(&claim, published_keys.iter().cloned());
        }
        Ok(())
    }
}

impl ShamirDb {
    /// Replace one subscription row in a single implicit tx. DELETE then SET
    /// makes SET's tx-aware preread miss the staged removal; `set_tx` writes
    /// the complete record rather than merging fields.
    pub(crate) async fn replace_subscription_row(
        &self,
        table: &crate::engine::table::TableManager,
        name: String,
        record: QueryValue,
    ) -> DbResult<()> {
        let interner = table.info_store_read_view().interner_read_view().await?;
        let mut stream = Box::pin(table.list_stream(128));
        let mut id = None;
        let mut stream_error = None;
        while let Some(batch) = stream.next().await {
            let batch = match batch {
                Ok(batch) => batch,
                Err(error) => {
                    stream_error = Some(error);
                    break;
                }
            };
            for (candidate, value) in batch {
                let inner = match value.into_inner() {
                    Ok(inner) => inner,
                    Err(error) => {
                        stream_error = Some(error);
                        break;
                    }
                };
                let row = interner
                    .inner_to_query_value(&inner)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                if row.get("name").and_then(|v| v.as_str()) == Some(name.as_str()) {
                    id = Some(candidate);
                    break;
                }
            }
            if id.is_some() || stream_error.is_some() {
                break;
            }
        }
        if let Some(error) = stream_error {
            return Err(error.into());
        }
        let id = id.ok_or_else(|| {
            shamir_storage::error::DbError::NotFound(format!(
                "replace_subscription_row: subscription '{name}' not found"
            ))
        })?;
        drop(stream);
        let repo = self.system_store().system_repo()?;
        let owned_table = table.clone();
        repo.run_implicit_batch_tx(Actor::System, "", move |tx| {
            Box::pin(async move {
                let interner = owned_table.interner_for_test().get().await?;
                let inner = shamir_types::codecs::interned::query_value_to_inner(&record, interner)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                let updated = owned_table.set_tx(id, &inner, Some(&mut *tx)).await?;
                Ok(crate::query::write::WriteResult {
                    affected: u64::from(updated),
                    records: Vec::new(),
                    execution_time_us: 0,
                })
            })
        })
        .await
        .map_err(|e| DbError::Storage(e.to_string()))?;
        Ok(())
    }
}
