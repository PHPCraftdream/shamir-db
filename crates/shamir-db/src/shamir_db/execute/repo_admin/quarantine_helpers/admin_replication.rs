//! Follower-side gap visibility (Part 2, RI-10): supervisor-facing seams on
//! [`ShamirDb`], moved here from `execute/admin_replication.rs` by PR6b-3 and
//! kept in sync with master's #188 lease/generation/quarantine semantics.
//!
//! `SubscriptionSupervisor` (`shamir-server`) only holds an `Arc<ShamirDb>` —
//! NOT a `ShamirAdminExecutor` — so these seams are exposed directly on
//! `ShamirDb` as internal system actions, without the authorization gate.

use crate::repl_binding::{quarantined_row_keys, SubscriptionBinding};
use crate::types::value::QueryValue;
use crate::{DbError, DbResult, ShamirDb};
use futures::StreamExt;
use shamir_types::access::Actor;
use shamir_types::mpack;

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
    ///
    /// WHO MAY CALL THE GENERATION-LESS VARIANT (round 12 F3): only callers
    /// that are NOT bound to a possibly-superseded binding generation —
    /// admin/API operations, and supervisor START-admission paths that read
    /// the row's current `binding_generation` synchronously under the
    /// replication control lock immediately before marking (the generation
    /// cannot go stale in between). Anything originating from a FOLLOWER
    /// LOOP task (or its supervisor exit wrapper) MUST use
    /// [`Self::mark_subscription_terminal_generation`] /
    /// [`Self::mark_subscription_resync_required_generation`] with its own
    /// lease/config generation: a stale task must never mark or clear a
    /// replacement binding's row. Retriable, NON-terminal status reports
    /// (stall visibility) must not use this writer at all — it quiesces the
    /// lease — and use [`Self::set_subscription_status_message`] instead.
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

    /// Round 12 F2 — the non-terminal, non-quiescing, generation-fenced
    /// status writer. Rewrites ONLY `status_message` on subscription
    /// `name`'s row (clears it when `message` is empty) and persists the
    /// catalogue. Deliberately does NOT touch `state`, `quarantined`,
    /// `resync_from_receipt` or `last_terminal_reason`, does NOT touch the
    /// quarantine cache, does NOT quiesce or invalidate the subscription's
    /// lease, and never changes the row's liveness: a retryable status
    /// report from a live follower must never invalidate the very lease
    /// that keeps the loop running.
    ///
    /// Missing row or `binding_generation != expected_binding_generation` →
    /// `Ok(())` as a logged no-op (a stale loop's stall report must never
    /// land on a replacement binding's row). Takes the replication control
    /// lock for the read-modify-write, like every other catalogue writer.
    pub async fn set_subscription_status_message(
        &self,
        name: &str,
        message: &str,
        expected_binding_generation: u64,
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
                "set_subscription_status_message: subscription '{name}' not found; \
                 treating as a no-op (it may have been dropped concurrently)"
            );
            return Ok(());
        };
        let mut record = row.as_value().into_owned();
        let row_generation = record.get("binding_generation").and_then(|v| v.as_u64());
        if row_generation != Some(expected_binding_generation) {
            log::warn!(
                "set_subscription_status_message: subscription '{name}' binding generation \
                 {row_generation:?} != expected {expected_binding_generation}; ignoring stale \
                 status report (a replacement binding must never be marked by an old task)"
            );
            return Ok(());
        }
        if let QueryValue::Map(ref mut m) = record {
            if message.is_empty() {
                m.insert("status_message".into(), QueryValue::Null);
            } else {
                m.insert(
                    "status_message".into(),
                    QueryValue::Str(message.to_string()),
                );
            }
        }
        self.replace_subscription_row(&table, name.to_string(), record)
            .await?;
        self.system_store()
            .persist_table_interner(&table, None)
            .await?;
        self.flush_seed_catalogue().await?;
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
            return Err(error);
        }
        let id = id.ok_or_else(|| {
            shamir_storage::error::DbError::NotFound(format!(
                "replace_subscription_row: subscription '{name}' not found"
            ))
        })?;
        drop(stream);
        let repo = self.system_store().system_repo()?;
        let permit = repo.write_domain().admit().await;
        let permit = permit.map_err(|e| DbError::Internal(e.to_string()))?;
        let inner = table.intern_query_value_admitted(&permit, &record).await?;
        drop(permit); // the tx commit admits again (non-reentrant domain)
        let owned_table = table.clone();
        repo.run_implicit_batch_tx(Actor::System, "", move |tx| {
            Box::pin(async move {
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
