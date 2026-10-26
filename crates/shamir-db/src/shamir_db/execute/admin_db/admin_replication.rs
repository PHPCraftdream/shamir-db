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
use crate::repl_binding::{row_is_quarantined, SubscriptionBinding, SUBSCRIPTION_BINDING_VERSION};
use crate::types::value::QueryValue;
use shamir_query_types::admin::ReplDirection;
use shamir_query_types::ReplicationServe;
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
        self.shamir
            .system_store()
            .flush_system_repo_durable(&table)
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
        self.shamir
            .system_store()
            .flush_system_repo_durable(&table)
            .await
            .map_err(|e| err(e.to_string()))?;
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
        self.shamir
            .system_store()
            .flush_system_repo_durable(&table)
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
        self.shamir
            .system_store()
            .flush_system_repo_durable(&table)
            .await
            .map_err(|e| err(e.to_string()))?;
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

        // PR6b-3: every concrete Pull destination of the profile must admit
        // repo-wide atomic batches BEFORE the subscription row exists.
        let profile_streams =
            super::replication_seed::catalogue::streams(&self.shamir, &op.profile)
                .await
                .ok();
        if let Some(streams) = &profile_streams {
            for stream in streams
                .iter()
                .filter(|stream| stream.direction == ReplDirection::Pull)
            {
                let Some(database) = self.shamir.get_db(&stream.scope.db) else {
                    continue;
                };
                for repo_name in database.list_repos() {
                    if stream
                        .scope
                        .repo
                        .as_ref()
                        .is_some_and(|repo| repo != &repo_name)
                    {
                        continue;
                    }
                    if let Some(destination) = database.get_repo(&repo_name) {
                        destination
                            .validate_repo_wide_atomic_batch(&format!(
                                "{}/{repo_name}",
                                stream.scope.db
                            ))
                            .map_err(|error| {
                                BatchError::query_coded(
                                    "",
                                    "atomic_batch_unsupported",
                                    error.to_string(),
                                )
                            })?;
                    }
                }
            }
        }

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
        self.shamir
            .system_store()
            .flush_system_repo_durable(&table)
            .await
            .map_err(|error| err(error.to_string()))?;
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
        if matches!(op.action, SubAction::Resume) {
            let profile = record
                .get("profile")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if let Ok(streams) =
                super::replication_seed::catalogue::streams(&self.shamir, profile).await
            {
                for stream in streams
                    .iter()
                    .filter(|stream| stream.direction == ReplDirection::Pull)
                {
                    let Some(database) = self.shamir.get_db(&stream.scope.db) else {
                        continue;
                    };
                    for repo_name in database.list_repos() {
                        if stream
                            .scope
                            .repo
                            .as_ref()
                            .is_some_and(|repo| repo != &repo_name)
                        {
                            continue;
                        }
                        if let Some(destination) = database.get_repo(&repo_name) {
                            destination
                                .validate_repo_wide_atomic_batch(&format!(
                                    "{}/{repo_name}",
                                    stream.scope.db
                                ))
                                .map_err(|error| {
                                    BatchError::query_coded(
                                        "",
                                        "atomic_batch_unsupported",
                                        error.to_string(),
                                    )
                                })?;
                        }
                    }
                }
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
        // successful Resume resolves this same name across older binding generations
        // only after the durable replace, interner flush, catalogue flush, and refresh.
        let resume_rollback = if matches!(op.action, SubAction::Resume) {
            SubscriptionBinding::from_row(&record).ok().map(|binding| {
                let keys = binding.destinations().to_vec();
                // Rebind increments generation; same-name pending claims from older generations
                // remain until recovery succeeds. Exact token rollback runs only after durability.
                let claims = self
                    .shamir
                    .resync_quarantine
                    .owner_claims_for_subscription(&name, &keys);
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
        self.shamir
            .system_store()
            .flush_system_repo_durable(&table)
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
