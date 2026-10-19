//! Open-time typed validation of the subscription catalogue + the binding
//! lifecycle guards (S5c-X PR8b-2): destination DROP/RENAME, same-name
//! subscription overwrite, quarantined-subscription DROP, and the
//! destination-preserving profile mutation guards.

use crate::query::admin::ReplStream;
use crate::repl_binding::binding::SubscriptionBinding;
use crate::repl_binding::coverage::row_is_quarantined;
use crate::repl_binding::pending::PENDING_STATE;
use crate::types::value::QueryValue;
use crate::{DbError, DbResult, ShamirDb};

/// One blocking subscription row, materialised while the binding guard
/// scans the catalogue so ONE typed error can name them ALL (round 5 B4).
pub(crate) struct BoundBlocker {
    pub(crate) name: String,
    pub(crate) generation: u64,
    pub(crate) quarantined: bool,
}

/// Build the typed R2/R3 refusal: names the operation, the destination and
/// every blocking subscription as `name (binding generation G, quarantined:
/// true|false)`, with the exact remedy text.
pub(crate) fn bound_error(
    operation: &str,
    db: &str,
    repo: Option<&str>,
    blockers: &[BoundBlocker],
) -> DbError {
    let mut listed = String::new();
    for blocker in blockers {
        listed.push_str(&format!(
            "\n  - {} (binding generation {}, quarantined: {})",
            blocker.name, blocker.generation, blocker.quarantined
        ));
    }
    DbError::ReplicationDestinationBound {
        detail: format!(
            "cannot {operation} destination '{db}/{}': referenced by {} subscription \
             binding(s):{listed}\nremedy: drop or reconfigure the referencing subscription(s) first",
            repo.unwrap_or("*"),
            blockers.len()
        ),
    }
}

/// Build the typed R1 refusal. Truthful remedy ONLY: in-place abandonment
/// of a quarantined subscription is not supported in this version.
pub(crate) fn quarantined_error(name: &str, db: &str, repo: &str) -> DbError {
    DbError::ReplicationDestinationQuarantined {
        detail: format!(
            "subscription '{name}' is quarantined (destination '{db}/{repo}'): \
             DROP SUBSCRIPTION is refused — in-place abandonment of a quarantined \
             subscription is not supported in this version"
        ),
    }
}

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
    /// Is `(db, repo)` covered by a quarantine-cache key that has not been
    /// durably confirmed yet (prepublished before a persist that may still
    /// fail, retained until the next successful refresh — round 5 B2)? The
    /// cache holds BOTH confirmed and pending keys; guards treat any key
    /// exactly like a durable binding (fail closed).
    fn destination_is_pending_quarantined(&self, db: &str, repo: Option<&str>) -> bool {
        match repo {
            Some(repo) => self.resync_quarantine.reason_for(db, repo).is_some(),
            // Database-wide target: any key under `db` counts.
            None => self.resync_quarantine.snapshot().entries.contains_key(db),
        }
    }

    /// Destination binding guard: refuse when any subscription row's durable
    /// binding references `(db, repo)` (`repo: None` = any repo in `db`) OR
    /// when the quarantine cache covers the destination (a prepublished key
    /// that the failed-persist window left undurable is just as binding —
    /// round 5 B2). `operation` is the ACTUAL operation string from the
    /// caller (round 6 item 8: "DROP", "RENAME", "DROP DATABASE CASCADE",
    /// "DROP REPO CASCADE") — the refusal must never name a different
    /// operation than the one attempted. The refusal is the typed
    /// [`DbError::ReplicationDestinationBound`] naming the operation, the
    /// destination and EVERY blocking subscription with its binding
    /// generation; the blocker's `quarantined` flag covers the HIT
    /// destination's cache-pending state, not just the row marker. Runs
    /// BEFORE any live or durable mutation of the destination.
    pub(crate) async fn ensure_destinations_unbound(
        &self,
        db: &str,
        repo: Option<&str>,
        operation: &str,
    ) -> DbResult<()> {
        // Collect ALL blocking rows while scanning so one error names them
        // all (round 5 B4).
        let mut blockers: Vec<BoundBlocker> = Vec::new();
        for row in read_subscription_rows(self).await? {
            let binding = SubscriptionBinding::from_row(&row)?;
            let name = row
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("<unnamed>")
                .to_string();
            // Round 6 item 8 / round 7 item 8: the pending-quarantine flag
            // is evaluated over EVERY matching destination of the row — a
            // FIRST-match-only check reported `quarantined: false` for a
            // healthy row whose SECOND matching destination is pending.
            let matches =
                |d: &(String, String)| d.0 == db && repo.is_none_or(|wanted| wanted == d.1);
            let hit = binding.destinations().iter().find(|d| matches(d));
            if hit.is_some() {
                let quarantined = row_is_quarantined(&row)
                    || binding.destinations().iter().any(|d| {
                        matches(d)
                            && self.destination_is_pending_quarantined(&d.0, Some(d.1.as_str()))
                    });
                blockers.push(BoundBlocker {
                    name,
                    generation: binding.generation(),
                    quarantined,
                });
            }
        }
        if !blockers.is_empty() {
            return Err(bound_error(operation, db, repo, &blockers));
        }
        // Round 5 (B2): no durable row blocks, but the quarantine cache
        // covers the destination — treat the pending key exactly like a
        // durable binding.
        if self.destination_is_pending_quarantined(db, repo) {
            return Err(DbError::ReplicationDestinationBound {
                detail: format!(
                    "cannot {operation} destination '{db}/{}': its quarantine key is \
                     prepublished (pending durable confirmation) in the replication \
                     quarantine cache; remedy: drop or reconfigure the referencing \
                     subscription(s) first",
                    repo.unwrap_or("*")
                ),
            });
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
    /// binding. EXCEPTION (round 2 A4): a row still at
    /// `pending_durable` is an unfinished creation barrier — no follower can
    /// have started on it — so retrying the creation overwrites it (logged).
    pub(crate) async fn ensure_subscription_absent(&self, name: &str) -> DbResult<()> {
        if let Some(row) = self.read_subscription_row(name).await? {
            if row.get("state").and_then(|value| value.as_str()) == Some(PENDING_STATE) {
                log::info!(
                    "subscription '{name}' is a pending_durable creation-barrier row; \
                     overwriting it (no follower can have started on it)"
                );
                return Ok(());
            }
            return Err(DbError::Validation(format!(
                "subscription '{name}' already exists: same-name overwrite is refused \
                 (S5c-X PR8b-2); use a new name"
            )));
        }
        Ok(())
    }

    /// Subscription-DROP guard: refuse while the row is quarantined — or
    /// while its bound destination is covered by a (possibly prepublished)
    /// quarantine-cache key (round 5 B2). Dropping is not evidence that its
    /// data became consistent; the typed
    /// [`DbError::ReplicationDestinationQuarantined`] carries ONLY the
    /// truthful remedy: in-place abandonment of a quarantined subscription
    /// is not supported in this version.
    pub(crate) async fn ensure_drop_allowed(&self, name: &str) -> DbResult<()> {
        let Some(row) = self.read_subscription_row(name).await? else {
            return Ok(());
        };
        // Round 6 item 8: consult EVERY bound destination, not only the
        // first — a quarantined/pending SECOND destination must refuse the
        // drop just like the first (a first-only check let the refusal slip
        // whenever the healthy destination happened to be listed first).
        let destinations = SubscriptionBinding::from_row(&row)?.destinations().to_vec();
        let checked = if destinations.is_empty() {
            vec![("<unknown>".to_string(), "<unknown>".to_string())]
        } else {
            destinations
        };
        for (db, repo) in &checked {
            if row_is_quarantined(&row)
                || self.destination_is_pending_quarantined(db, Some(repo.as_str()))
            {
                return Err(quarantined_error(name, db, repo));
            }
        }
        Ok(())
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
        // `from_streams` returns the destination Vec directly, not a binding.
        if bound.destinations() != proposed.as_slice() {
            return Err(DbError::Validation(format!(
                "cannot set profile '{new_profile}': its Pull destinations {proposed:#?} differ \
                 from the bound destinations {:#?}",
                bound.destinations()
            )));
        }
        Ok(())
    }
}
