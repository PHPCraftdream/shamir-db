//! [`TransactionInfo`] — MVCC transaction metadata in batch responses.

use serde::{Deserialize, Serialize};

/// Transaction metadata returned in batch responses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionInfo {
    /// Transaction ID (monotonic per repo).
    pub tx_id: u64,

    /// Outcome: `"committed"` or `"aborted"`.
    pub status: String,

    /// Human-readable abort reason (null when committed).
    /// E.g. `"tx_conflict"`, `"tx_cross_repo_not_supported"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// MVCC snapshot version the tx read from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_version: Option<u64>,

    /// Committed version (null when aborted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_version: Option<u64>,

    /// Whether the commit's projections (data → main store, counters,
    /// secondary indexes, HNSW graph) materialized inline on the commit
    /// path.
    ///
    /// - `true` (the common case): every projection landed before the
    ///   response — the committed version is fully observable now.
    /// - `false`: the commit is durable (its WAL entry IS the commit),
    ///   but at least one projection was deferred to crash-recovery,
    ///   which re-applies it idempotently on the next open. The data
    ///   WILL appear — this is restart-bounded eventual consistency, not
    ///   an abort.
    ///
    /// Only meaningful when `status == "committed"`. Defaults to `true`
    /// so payloads serialized before this field existed (and clients
    /// that omit it) deserialize to the fully-materialized common case.
    #[serde(default = "default_materialized")]
    pub materialized: bool,
}

fn default_materialized() -> bool {
    true
}

impl TransactionInfo {
    pub fn committed(
        tx_id: u64,
        snapshot_version: u64,
        commit_version: u64,
        materialized: bool,
    ) -> Self {
        Self {
            tx_id,
            status: "committed".into(),
            reason: None,
            snapshot_version: Some(snapshot_version),
            commit_version: Some(commit_version),
            materialized,
        }
    }

    pub fn aborted(tx_id: u64, reason: impl Into<String>) -> Self {
        Self {
            tx_id,
            status: "aborted".into(),
            reason: Some(reason.into()),
            snapshot_version: None,
            commit_version: None,
            // Aborted txs never materialize; default is irrelevant to the
            // client because `status` already disambiguates.
            materialized: true,
        }
    }

    pub fn is_committed(&self) -> bool {
        self.status == "committed"
    }
}
