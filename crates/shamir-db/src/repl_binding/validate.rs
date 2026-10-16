//! Open-time typed validation of the subscription catalogue + the binding
//! lifecycle guards (S5c-X PR8b-2): destination DROP/RENAME, same-name
//! subscription overwrite, quarantined-subscription DROP, and the
//! destination-preserving profile mutation guards.

use crate::query::admin::ReplStream;
use crate::repl_binding::binding::SubscriptionBinding;
use crate::repl_binding::coverage::row_is_quarantined;
use crate::types::value::QueryValue;
use crate::{DbError, DbResult, ShamirDb};

/// Read every `system/subscriptions` row as an owned [`QueryValue`] — the
/// standard system-catalogue read pattern (mirrors
/// `admin_replication.rs::read_all`).
pub(crate) async fn read_subscription_rows(shamir: &ShamirDb) -> DbResult<Vec<QueryValue>> {
    let table = shamir.system_store().subscriptions_table().await?;
    let refs = crate::types::common::new_map();
    let ctx = table.info_store_read_view().filter_context(&refs).await?;
    let result = table
        .read(&crate::query::read::ReadQuery::new("subscriptions"), &ctx)
        .await?;
    Ok(result
        .records
        .into_iter()
        .map(|record| record.as_value().into_owned())
        .collect())
}

/// Validate EVERY subscription row — active, paused, quarantined alike —:
/// required string fields plus a fully valid durable binding. The first
/// failure is returned as the typed
/// [`DbError::SubscriptionCatalogue`] and open aborts (the init seam calls
/// this through `refresh_resync_quarantine_cache` before returning the
/// [`ShamirDb`]).
pub async fn validate_subscription_catalogue(shamir: &ShamirDb) -> DbResult<()> {
    for row in read_subscription_rows(shamir).await? {
        let fail = |reason: String| {
            let name = row
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("<unnamed>");
            DbError::subscription_catalogue(name, reason)
        };
        match row.get("name").and_then(|value| value.as_str()) {
            Some(name) if !name.is_empty() => {}
            _ => {
                return Err(DbError::subscription_catalogue(
                    "<unnamed>",
                    "missing or empty 'name'",
                ))
            }
        }
        for field in ["upstream", "publication"] {
            if !matches!(row.get(field), Some(QueryValue::Str(_))) {
                return Err(fail(format!("missing or non-string '{field}'")));
            }
        }
        for field in ["profile", "state"] {
            match row.get(field).and_then(|value| value.as_str()) {
                Some(value) if !value.is_empty() => {}
                _ => return Err(fail(format!("missing or empty '{field}'"))),
            }
        }
        SubscriptionBinding::from_row(&row)?;
    }
    Ok(())
}

impl ShamirDb {
    /// DROP/RENAME guard: refuse when any subscription row's durable binding
    /// references `(db, repo)` (`repo: None` = any repo in `db`). Rows that
    /// cannot be parsed propagate the typed catalogue error. Runs BEFORE any
    /// live or durable mutation of the destination.
    pub(crate) async fn ensure_destinations_unbound(
        &self,
        db: &str,
        repo: Option<&str>,
    ) -> DbResult<()> {
        for row in read_subscription_rows(self).await? {
            let binding = SubscriptionBinding::from_row(&row)?;
            let name = row
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("<unnamed>");
            for (bound_db, bound_repo) in binding.destinations() {
                if bound_db == db && repo.is_none_or(|wanted| wanted == bound_repo) {
                    return Err(DbError::Validation(format!(
                        "destination '{db}/{}' is referenced by subscription '{name}' binding; \
                         drop the subscription (it must not be quarantined) before renaming \
                         or removing the destination",
                        bound_repo
                    )));
                }
            }
        }
        Ok(())
    }

    /// Read the single subscription row named `name`, if any.
    pub(crate) async fn read_subscription_row(&self, name: &str) -> DbResult<Option<QueryValue>> {
        let table = self.system_store().subscriptions_table().await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let existing = table
            .read(
                &crate::query::read::ReadQuery::new("subscriptions").filter(
                    crate::query::filter::Filter::Eq {
                        field: vec!["name".to_string()],
                        value: crate::query::filter::FilterValue::String(name.to_string()),
                    },
                ),
                &ctx,
            )
            .await?;
        Ok(existing
            .records
            .into_iter()
            .next()
            .map(|record| record.as_value().into_owned()))
    }
}

impl ShamirDb {
    /// Same-name overwrite guard: refuse CREATE SUBSCRIPTION when a row with
    /// the name already exists — a replacement binding must never silently
    /// detach quarantine coverage from data already written under the old
    /// binding.
    pub(crate) async fn ensure_subscription_absent(&self, name: &str) -> DbResult<()> {
        if self.read_subscription_row(name).await?.is_some() {
            return Err(DbError::Validation(format!(
                "subscription '{name}' already exists: same-name overwrite is refused \
                 (S5c-X PR8b-2); use a new name"
            )));
        }
        Ok(())
    }

    /// Subscription-DROP guard: refuse while the row is quarantined —
    /// dropping is not evidence that its data became consistent; follow the
    /// reseed runbook. Typed `resync_required` error naming the bound
    /// destination.
    pub(crate) async fn ensure_drop_allowed(&self, name: &str) -> DbResult<()> {
        let Some(row) = self.read_subscription_row(name).await? else {
            return Ok(());
        };
        if !row_is_quarantined(&row) {
            return Ok(());
        }
        let (db, repo) = SubscriptionBinding::from_row(&row)?
            .destinations()
            .first()
            .cloned()
            .unwrap_or_else(|| ("<unknown>".to_string(), "<unknown>".to_string()));
        Err(DbError::resync_required(
            &db,
            &repo,
            format!(
                "subscription '{name}' is quarantined (resync_required): dropping is refused \
                 — dropping is not evidence of consistency; follow guide/09-seed-recovery.md \
                 to reseed"
            ),
        ))
    }

    /// Profile-upsert guard: when `profile` already exists and is referenced
    /// by subscription rows, the proposed streams' Pull destination set must
    /// equal every referencing row's bound destinations. Rows not
    /// referencing the profile are unaffected.
    pub(crate) async fn ensure_profile_binding_unchanged(
        &self,
        profile: &str,
        proposed: &[ReplStream],
    ) -> DbResult<()> {
        for row in read_subscription_rows(self).await? {
            if row.get("profile").and_then(|v| v.as_str()) != Some(profile) {
                continue;
            }
            // Resolve the proposed destination set lazily: only a
            // referencing row needs it.
            let destinations = SubscriptionBinding::from_streams(profile, proposed)?;
            let bound = SubscriptionBinding::from_row(&row)?;
            if bound.destinations() != destinations.as_slice() {
                let name = row
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<unnamed>");
                return Err(DbError::Validation(format!(
                    "cannot recreate replication profile '{profile}': subscription '{name}' \
                     is bound to destinations {:#?} but the proposed profile has Pull \
                     destinations {destinations:#?}",
                    bound.destinations()
                )));
            }
        }
        Ok(())
    }

    /// SetProfile guard: the proposed profile's (already resolved) Pull
    /// destination set must EXACTLY equal `row`'s durable binding.
    pub(crate) async fn ensure_set_profile_binding_unchanged(
        &self,
        row: &QueryValue,
        new_profile: &str,
        proposed_streams: &[ReplStream],
    ) -> DbResult<()> {
        let proposed = SubscriptionBinding::from_streams(new_profile, proposed_streams)?;
        let bound = SubscriptionBinding::from_row(row)?;
        if bound.destinations() != proposed.destinations() {
            return Err(DbError::Validation(format!(
                "cannot set profile '{new_profile}': its Pull destinations {:#?} differ from \
                 the bound destinations {:#?}",
                proposed.destinations(),
                bound.destinations()
            )));
        }
        Ok(())
    }
}
