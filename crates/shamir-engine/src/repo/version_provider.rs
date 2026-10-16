use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bytes::Bytes;
use shamir_tx::{MvccStore, VersionProvider};
use shamir_types::types::common::THasher;

use crate::table::table_manager::table_token_for;

pub struct RepoVersionProvider {
    pub per_table_mvcc: Arc<scc::HashMap<u64, Arc<MvccStore>, THasher>>,
}

impl VersionProvider for RepoVersionProvider {
    fn version_of(&self, table_id: u64, key: &Bytes) -> Option<u64> {
        self.per_table_mvcc
            .read_sync(&table_id, |_, mvcc| mvcc.version_of(key.as_ref()))
        // Returns None when table_id not in per_table_mvcc map
        // — propagates as conflict in validate_read_set.
    }
}

/// Convert the wire DTO [`shamir_query_types::admin::Retention`] into the
/// engine-internal [`shamir_tx::Retention`]. The orphan rule forbids a
/// cross-crate `From` impl, so this free fn bridges the two types.
///
/// Call `Retention::validate()` on the DTO first; this fn performs no
/// validation (it is a pure field copy).
pub fn to_mvcc_retention(r: &shamir_query_types::admin::Retention) -> shamir_tx::Retention {
    shamir_tx::Retention {
        max_age_secs: r.max_age_secs,
        max_count: r.max_count,
        min_count: r.min_count,
    }
}

/// Deterministic u64 token for a repository name.
///
/// Stage 4: `DefaultHasher(name)` placeholder.
/// Stage 5: real repo-level interner ID.
pub fn repo_token(name: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
}

/// Record `table_token_for(name) → name` in the reverse index, detecting
/// the two non-trivial cases explicitly:
///
/// * **Idempotent re-registration** — the same name added twice (e.g.
///   `create_table` issued twice, or catalogue reload over an existing
///   config). The existing entry already equals `name`, so this is a
///   no-op.
/// * **Token collision** — two *distinct* names that hash to the same
///   `u64` token. With a 64-bit `DefaultHasher` over table names this is
///   astronomically unlikely, but if it ever happens we keep the FIRST
///   registration (do not clobber a live mapping) and log a warning so the
///   situation is visible instead of silently corrupting `table_by_token`
///   for the first table. The second table is then unresolvable by token —
///   the caller should rename it.
pub(crate) fn register_token(token_names: &scc::HashMap<u64, String, THasher>, name: &str) {
    let token = table_token_for(name);
    if let Err((_, attempted)) = token_names.insert_sync(token, name.to_string()) {
        // Key already present — inspect the existing mapping.
        let existing = token_names.read_sync(&token, |_, n| n.clone());
        if existing.as_deref() != Some(attempted.as_str()) {
            log::warn!(
                "repo_instance: table token collision on {} — keeping '{}', \
                 refusing '{}'; the latter is unresolvable by token",
                token,
                existing.as_deref().unwrap_or("<unknown>"),
                attempted
            );
        }
    }
}
