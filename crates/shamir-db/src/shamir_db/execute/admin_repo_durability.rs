//! Admin handlers: AlterRepo, DescribeRepo (per-repo durability, S5c-X
//! PR1b part 2). Lives OUTSIDE the over-limit `admin_db_repo.rs` per the
//! folder-entry ratchet.

use core::num::NonZeroU64;

use crate::access::{Action, ResourcePath};
use crate::engine::repo::durability_policy::{effective_wal_segment_bytes, resolve};
use crate::engine::repo::DurabilityPolicy;
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::shamir_db::repo_durability::{
    durability_from_record, durability_to_record, merge_override,
};
use crate::types::value::QueryValue;
use shamir_query_types::admin::{AlterRepoOp, DescribeRepoOp};
use shamir_query_types::durability::DurabilityOverride;
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, validate_name_component};

fn err_code(code: &str, msg: impl Into<String>) -> BatchError {
    BatchError::QueryError {
        alias: String::new(),
        message: msg.into(),
        code: Some(code.to_string()),
    }
}

fn err(msg: impl Into<String>) -> BatchError {
    BatchError::QueryError {
        alias: String::new(),
        message: msg.into(),
        code: None,
    }
}

/// Effective policy → response map (same key spellings as the stored
/// override record; `wal_commit` is `effective_wal_commit()` — the tier
/// that actually applies on this backend).
fn effective_durability_record(policy: &DurabilityPolicy) -> QueryValue {
    let over = DurabilityOverride {
        wal_commit: Some(policy.effective_wal_commit()),
        wal_retention_max_bytes: NonZeroU64::new(policy.wal_retention_max_bytes()),
        on_wal_retention_exceeded: Some(policy.on_wal_retention_exceeded()),
        replication_serve: Some(policy.replication_serve()),
    };
    durability_to_record(&over)
}

impl ShamirAdminExecutor {
    /// ALTER REPO — patch the per-repo durability override (fields `Some`
    /// in the op win, `None` keep the stored value), validate the MERGED
    /// override against the server default, live-swap the open repo's
    /// policy through `set_durability`, and persist the merged override in
    /// the catalogue record. No HMAC: this is a settings change, not a
    /// destructive drop.
    pub(super) async fn handle_alter_repo(
        &self,
        op: &AlterRepoOp,
    ) -> Result<QueryResult, BatchError> {
        validate_name_component(&op.alter_repo, "repo_name")?;

        // Auth: Write on the repo (settings mutation). Mirrors rename_repo.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::store(self.db_name.clone(), op.alter_repo.clone()),
                Action::Write,
            )
            .await
            .map_err(|e| err_code("access_denied", e.to_string()))?;

        // Serialize CREATE/ALTER merges with the established per-db admin
        // lock. Clone the Arc out of the DashMap entry before awaiting; no
        // concurrent-map guard crosses an await.
        let alter_lock = self
            .shamir
            .repo_create_locks()
            .entry(self.db_name.clone())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _alter_guard = alter_lock.lock().await;

        // Existence probe (rename_repo's typed error shape).
        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        if !db.has_repo(&op.alter_repo) {
            return Err(err(format!(
                "Repository '{}/{}' not found",
                self.db_name, op.alter_repo
            )));
        }
        let record = self
            .shamir
            .system_store()
            .load_repository_record(&self.db_name, &op.alter_repo)
            .await
            .map_err(|e| err(e.to_string()))?
            .ok_or_else(|| {
                err(format!(
                    "repository catalogue record for '{}/{}' not found",
                    self.db_name, op.alter_repo
                ))
            })?;

        let repo = db.get_repo(&op.alter_repo).ok_or_else(|| {
            err(format!(
                "Repository '{}/{}' not found",
                self.db_name, op.alter_repo
            ))
        })?;

        // Patch semantics: decode the stored override (absent = default),
        // merge, validate the MERGED value — invalid ⇒ typed error with NO
        // partial effect.
        let base = durability_from_record(&record)
            .map_err(|e| err(e.to_string()))?
            .unwrap_or_default();
        let patch = op.durability.unwrap_or_default();
        let merged = merge_override(&base, &patch);
        let server_default = self.shamir.durability_policy();
        let resolved = repo
            .validate_durability_change(&server_default, &merged)
            .map_err(|e| {
                let code = e.code().unwrap_or("invalid_request");
                err_code(code, e.to_string())
            })?;

        // Preserve engine/path/meta and persist the MERGED override FIRST.
        // The per-db admin lock is held across the read/validate/persist/swap
        // sequence; repo policy's ArcSwap store is synchronous and is the
        // final operation, so concurrent alters cannot reorder durable/live
        // state or lose a patch.
        let engine = record
            .get("engine")
            .and_then(|v| v.as_str())
            .unwrap_or("in_memory")
            .to_string();
        let path = record
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let meta = crate::access::ResourceMeta::from_record(&record);
        self.shamir
            .system_store()
            .save_repository(
                &self.db_name,
                &op.alter_repo,
                &engine,
                path.as_deref(),
                &meta,
                Some(&merged),
            )
            .await
            .map_err(|e| err(e.to_string()))?;

        // Live swap after successful persistence. The engine prepared path
        // cannot fail: policy validation used the identical captured segment;
        // only backend normalization and ArcSwap::store remain.
        repo.set_durability(resolved)
            .map_err(|e| err(e.to_string()))?;

        let effective = repo.durability_policy();
        if let Some(observer) = &self.repo_admin_observer {
            observer.repository_durability_changed(&self.db_name, &op.alter_repo, &effective);
        }
        Ok(admin_result(mpack!({
            "altered_repo": @(QueryValue::Str(op.alter_repo.clone())),
            "altered": @(QueryValue::Bool(true)),
            "durability": @(effective_durability_record(&effective)),
        })))
    }

    /// DESCRIBE REPO — full form: engine, path, EFFECTIVE durability policy
    /// (live from the open repo) and the stored override (so operators see
    /// what is inherited vs overridden).
    pub(super) async fn handle_describe_repo(
        &self,
        op: &DescribeRepoOp,
    ) -> Result<QueryResult, BatchError> {
        validate_name_component(&op.describe_repo, "repo_name")?;

        // Authz: Action::Read on the repo (introspection). Mirrors
        // describe_table.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::store(self.db_name.clone(), op.describe_repo.clone()),
                Action::Read,
            )
            .await
            .map_err(|e| err_code("access_denied", e.to_string()))?;

        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        if !db.has_repo(&op.describe_repo) {
            return Err(err(format!(
                "Repository '{}/{}' not found",
                self.db_name, op.describe_repo
            )));
        }
        let record = self
            .shamir
            .system_store()
            .load_repository_record(&self.db_name, &op.describe_repo)
            .await
            .map_err(|e| err(e.to_string()))?
            .ok_or_else(|| {
                err(format!(
                    "repository catalogue record for '{}/{}' not found",
                    self.db_name, op.describe_repo
                ))
            })?;

        let engine = record
            .get("engine")
            .and_then(|v| v.as_str())
            .unwrap_or("in_memory")
            .to_string();
        let path_qv = record.get("path").cloned().unwrap_or(QueryValue::Null);
        let stored_override = durability_from_record(&record)
            .map_err(|e| err(e.to_string()))?
            .map(|over| durability_to_record(&over))
            .unwrap_or(QueryValue::Null);

        // Describe live state without initializing recovery or transaction
        // machinery; a repo that has already initialized its fail-stop gate
        // may be described, but an ALTER refuses that state before writes.
        let effective_qv = match db.get_repo(&op.describe_repo) {
            Some(repo) => {
                let policy = repo.durability_policy();
                effective_durability_record(&policy)
            }
            None => {
                let over = durability_from_record(&record)
                    .map_err(|e| err(e.to_string()))?
                    .unwrap_or_default();
                let policy = resolve(
                    &self.shamir.durability_policy(),
                    Some(&over),
                    effective_wal_segment_bytes(),
                )
                .map_err(|e| err(e.to_string()))?;
                effective_durability_record(&policy)
            }
        };

        Ok(admin_result(mpack!({
            "describe_repo": @(QueryValue::Str(op.describe_repo.clone())),
            "repo": @(QueryValue::Str(self.db_name.clone())),
            "engine": @(QueryValue::Str(engine)),
            "path": @(path_qv),
            "durability": @(effective_qv),
            "durability_override": @(stored_override),
        })))
    }
}
