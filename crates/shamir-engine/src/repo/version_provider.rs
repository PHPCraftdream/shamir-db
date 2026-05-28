use std::sync::Arc;

use bytes::Bytes;
use shamir_tx::{MvccStore, VersionProvider};

pub struct RepoVersionProvider {
    pub per_table_mvcc: Arc<scc::HashMap<u64, Arc<MvccStore>>>,
}

impl VersionProvider for RepoVersionProvider {
    fn version_of(&self, table_id: u64, key: &Bytes) -> u64 {
        self.per_table_mvcc
            .read(&table_id, |_, mvcc| mvcc.version_of(key.as_ref()))
            .unwrap_or(0)
    }
}
