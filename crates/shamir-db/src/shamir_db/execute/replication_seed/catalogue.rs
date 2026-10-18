use crate::query::admin::{CreateSubscriptionOp, ReplStream};
use crate::query::read::ReadQuery;
use crate::query::write::SetOp;
use crate::repl_binding::pending::next_generation;
use crate::repl_binding::{SubscriptionBinding, SUBSCRIPTION_BINDING_VERSION};
use crate::types::value::QueryValue;
use crate::{DbResult, ShamirDb};
use shamir_engine::repo::{RepoInstance, SeedInstallState, SeedReceipt};
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_types::mpack;

use super::SeedRefusal;

pub(super) async fn rows(home: &ShamirDb, name: &str) -> DbResult<Vec<QueryValue>> {
    let table = match name {
        "subscriptions" => home.system_store().subscriptions_table().await?,
        "publications" => home.system_store().publications_table().await?,
        "replication_profiles" => home.system_store().replication_profiles_table().await?,
        _ => {
            return Err(crate::DbError::Validation(
                "unknown replication catalogue".into(),
            ))
        }
    };
    let refs = crate::types::common::new_map();
    let ctx = table.info_store_read_view().filter_context(&refs).await?;
    Ok(table
        .read(&ReadQuery::new(name), &ctx)
        .await?
        .records
        .into_iter()
        .map(|row| row.as_value().into_owned())
        .collect())
}

pub(in crate::shamir_db::execute) async fn streams(
    home: &ShamirDb,
    profile: &str,
) -> Result<Vec<ReplStream>, SeedRefusal> {
    let records = rows(home, "replication_profiles")
        .await
        .map_err(|_| SeedRefusal::WrongScope)?;
    let value = records
        .iter()
        .find(|row| row["name"].as_str() == Some(profile))
        .and_then(|row| row.get("streams"))
        .ok_or(SeedRefusal::WrongScope)?;
    let bytes = rmp_serde::to_vec_named(value).map_err(|_| SeedRefusal::WrongScope)?;
    rmp_serde::from_slice(&bytes).map_err(|_| SeedRefusal::WrongScope)
}

pub(super) fn matches_scope(streams: &[ReplStream], receipt: &SeedReceipt) -> bool {
    streams.len() == 1
        && streams[0].direction == ReplDirection::Pull
        && streams[0].mode == ReplMode::ReadOnly
        && streams[0].scope.db == receipt.source_db
        && streams[0].scope.repo.as_deref() == Some(receipt.source_repo.as_str())
        && streams[0].scope.table.is_none()
}

pub(super) async fn receipt(repo: &RepoInstance) -> Result<Option<SeedReceipt>, SeedRefusal> {
    let info = repo
        .tx_info_store()
        .await
        .map_err(|_| SeedRefusal::IncompleteInstall)?;
    match SeedInstallState::load(&info)
        .await
        .map_err(|_| SeedRefusal::IncompleteInstall)?
    {
        Some(SeedInstallState::Installing(_)) => Err(SeedRefusal::IncompleteInstall),
        Some(SeedInstallState::Complete(receipt)) => {
            SeedInstallState::require_complete(&info, true)
                .await
                .map_err(|_| SeedRefusal::IncompleteInstall)?;
            Ok(Some(receipt))
        }
        None => Ok(None),
    }
}

pub(super) async fn persist(home: &ShamirDb, record: QueryValue) -> DbResult<()> {
    let name = record["name"]
        .as_str()
        .ok_or_else(|| crate::DbError::Validation("subscription name missing".into()))?
        .to_owned();
    let table = home.system_store().subscriptions_table().await?;
    let op = SetOp {
        set: crate::query::TableRef::new("subscriptions"),
        key: mpack!({"name": @(QueryValue::Str(name))}),
        value: record,
    };
    home.system_store().set_via_implicit_tx(&table, &op).await?;
    home.system_store()
        .persist_table_interner(&table, None)
        .await?;
    home.flush_seed_catalogue().await
}

pub(super) fn binding(record: &mut QueryValue, receipt: &SeedReceipt) {
    if let QueryValue::Map(fields) = record {
        fields.insert("seed_receipt".into(), super::super::helpers::to_qv(receipt));
    }
}

impl ShamirDb {
    /// Offline filesystem-authorized profile step of a seed install. An existing
    /// profile is accepted only when its streams are exactly `streams`.
    pub async fn install_seed_profile(&self, name: &str, streams: &[ReplStream]) -> DbResult<()> {
        let wanted = super::super::helpers::to_qv(&streams);
        let records = rows(self, "replication_profiles").await?;
        if let Some(existing) = records
            .iter()
            .find(|row| row["name"].as_str() == Some(name))
        {
            if existing.get("streams") != Some(&wanted) {
                return Err(crate::DbError::Validation(format!(
                    "{}: existing profile differs",
                    SeedRefusal::WrongScope.code()
                )));
            }
            return self.flush_seed_catalogue().await;
        }
        let mut fields = crate::types::common::new_map();
        fields.insert("name".into(), QueryValue::Str(name.to_owned()));
        fields.insert("streams".into(), wanted);
        let table = self.system_store().replication_profiles_table().await?;
        let op = SetOp {
            set: crate::query::TableRef::new("replication_profiles"),
            key: mpack!({"name": @(QueryValue::Str(name.to_owned()))}),
            value: QueryValue::Map(fields),
        };
        self.system_store().set_via_implicit_tx(&table, &op).await?;
        self.system_store()
            .persist_table_interner(&table, None)
            .await?;
        self.flush_seed_catalogue().await
    }

    /// Offline filesystem-authorized final install step. Three branches on an
    /// existing row of the same name: (1) EXACT-RETRY — binding, upstream,
    /// publication, profile and `seed_receipt` all already match ⇒ flush only;
    /// (2) REPLACEMENT (S5c-X verdict evidence row 9, quarantined-DROP is
    /// refused) — the row is durably `quarantined` and the NEW receipt binds
    /// on the SAME scope (upstream/publication/profile and binding must match
    /// exactly): only the `seed_receipt` is rewritten, preserving `state`, the
    /// `quarantined` marker, `resync_from_receipt`, `status_message` and
    /// `last_terminal_reason` — only a post-reseed `Resume` may clear the
    /// quarantine; (3) anything else ⇒ `ConflictingBinding` (a healthy row's
    /// receipt is never overwritten). A new name creates a fresh paused row.
    ///
    /// Round 2 (A5): the WHOLE install runs under `replication_control_lock`
    /// — an offline install writes the subscription catalogue and must not
    /// race destination lifecycle ops (`remove_db`/`remove_repo`/renames,
    /// which take the same lock before their binding guard).
    pub async fn install_seed_subscription(
        &self,
        image: &RepoInstance,
        expected: &SeedReceipt,
        op: &CreateSubscriptionOp,
    ) -> Result<(), SeedRefusal> {
        let _control = self.replication_control_lock.lock().await;
        if !cfg!(unix) {
            return Err(SeedRefusal::IncompleteInstall);
        }
        let actual = receipt(image)
            .await?
            .ok_or(SeedRefusal::OrdinaryRepository)?;
        if actual != *expected || !op.paused {
            return Err(SeedRefusal::ConflictingBinding);
        }
        let scope = streams(self, &op.profile).await?;
        if !matches_scope(&scope, expected) {
            return Err(SeedRefusal::WrongScope);
        }
        // S5c-X PR8b-2: the seed-installed row carries the same durable
        // binding an ordinary creation would write; the GENERATION is stamped
        // per-branch below (monotonic — an exact retry must not be rejected
        // because the row's generation advanced).
        let destinations = SubscriptionBinding::from_streams(&op.create_subscription, &scope)
            .map_err(|_| SeedRefusal::WrongScope)?;
        let records = rows(self, "subscriptions")
            .await
            .map_err(|_| SeedRefusal::ConflictingBinding)?;
        if let Some(existing) = records
            .iter()
            .find(|row| row["name"].as_str() == Some(op.create_subscription.as_str()))
        {
            let existing_binding = SubscriptionBinding::from_row(existing)
                .map_err(|_| SeedRefusal::ConflictingBinding)?;
            let scope_matches = existing["upstream"].as_str() == Some(op.upstream.as_str())
                && existing["publication"].as_str() == Some(op.publication.as_str())
                && existing["profile"].as_str() == Some(op.profile.as_str());
            // Exact-retry branch (round 2 A5): the existing row already
            // carries the SAME scope + destinations + receipt. Destinations
            // are compared, NOT full binding equality — generations change
            // across replacement rebinds and a legitimate retry must still
            // match.
            if scope_matches
                && existing_binding.destinations() == destinations
                && existing.get("seed_receipt") == Some(&super::super::helpers::to_qv(expected))
            {
                return self
                    .flush_seed_catalogue()
                    .await
                    .map_err(|_| SeedRefusal::ConflictingBinding);
            }
            // REPLACEMENT branch (S5c-X verdict evidence row 9): the row is
            // durably quarantined — DROP is refused, so rebinding the NEW
            // receipt onto the EXISTING row is the only sanctioned recovery.
            // A replacement seed must serve exactly the same bound
            // destinations, so scope AND destinations must match; `op.paused`
            // was enforced at the top. NO `check_owners` call (round 2 A5):
            // the sibling ownership predates the rebind — this row's binding
            // was accepted with that sibling already in place, and re-proving
            // ownership now would block sanctioned recovery. Everything but
            // `seed_receipt` (and a bumped generation) is preserved: only a
            // post-reseed `Resume` may lift the quarantine. The bump is at
            // least +1 over the row's current generation AND above the
            // durable MVCC-version floor, so a rebind also beats any
            // concurrent allocation.
            if crate::repl_binding::row_is_quarantined(existing) {
                if !scope_matches || existing_binding.destinations() != destinations {
                    return Err(SeedRefusal::ConflictingBinding);
                }
                let replacement_binding = SubscriptionBinding::new(
                    SUBSCRIPTION_BINDING_VERSION,
                    std::cmp::max(
                        existing_binding.generation() + 1,
                        next_generation(self, &records)
                            .await
                            .map_err(|_| SeedRefusal::ConflictingBinding)?,
                    ),
                    destinations.clone(),
                )
                .map_err(|_| SeedRefusal::ConflictingBinding)?;
                let mut record = existing.clone();
                binding(&mut record, expected);
                // Replacement-seed rebind: invalidate the old generation's
                // lease (quiesce) before the row changes.
                self.quiesce_subscription_lease_locked(&op.create_subscription, &_control)
                    .await;
                replacement_binding.write_to_row(&mut record);
                return persist(self, record)
                    .await
                    .map_err(|_| SeedRefusal::ConflictingBinding);
            }
            // Never overwrite a healthy row's receipt.
            return Err(SeedRefusal::ConflictingBinding);
        }
        super::control::check_owners(self, &op.create_subscription, expected).await?;
        // New row (round 2 A5): monotonic generation across the whole
        // catalogue, not a hardcoded 1.
        let new_binding = SubscriptionBinding::new(
            SUBSCRIPTION_BINDING_VERSION,
            next_generation(self, &records)
                .await
                .map_err(|_| SeedRefusal::ConflictingBinding)?,
            destinations,
        )
        .map_err(|_| SeedRefusal::ConflictingBinding)?;
        let mut fields = crate::types::common::new_map();
        fields.insert(
            "name".into(),
            QueryValue::Str(op.create_subscription.clone()),
        );
        fields.insert("upstream".into(), QueryValue::Str(op.upstream.clone()));
        fields.insert(
            "publication".into(),
            QueryValue::Str(op.publication.clone()),
        );
        fields.insert("profile".into(), QueryValue::Str(op.profile.clone()));
        fields.insert("state".into(), QueryValue::Str("paused".into()));
        let mut record = QueryValue::Map(fields);
        binding(&mut record, expected);
        new_binding.write_to_row(&mut record);
        persist(self, record)
            .await
            .map_err(|_| SeedRefusal::ConflictingBinding)
    }
}
