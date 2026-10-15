use std::sync::Arc;

use bytes::Bytes;
use shamir_tx::{MvccStore, VersionProvider};
use shamir_types::types::common::THasher;

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
