//! [`TransactionInfo`] — MVCC transaction metadata in batch responses.

use serde::{Deserialize, Deserializer, Serialize};

use crate::batch::VisibilityState;

/// Transaction metadata returned in batch responses.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransactionInfo {
    /// Transaction ID (monotonic per repo).
    pub tx_id: u64,

    /// Outcome: `"committed"`, `"aborted"`, or `"indeterminate"`.
    pub status: String,

    /// Human-readable abort reason (null when committed; omitted for
    /// indeterminate, whose `error` is `"wal_outcome_unknown"`).
    /// E.g. `"tx_conflict"`, `"tx_cross_repo_not_supported"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Published commit error; does not change committed status or abort reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// MVCC snapshot version the tx read from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_version: Option<u64>,

    /// Committed version for committed outcomes, the ATTEMPTED version (no
    /// success claim) for indeterminate outcomes, absent for aborted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_version: Option<u64>,

    /// Whether the commit's projections (data → main store, counters,
    /// secondary indexes, HNSW graph) materialized inline on the commit
    /// path.
    ///
    /// Inline projections do not establish repository-prefix visibility.
    /// Inspect `visibility` independently, including on reopen.
    ///
    /// Only meaningful when `status == "committed"`. Defaults to `true`
    /// so payloads serialized before this field existed (and clients
    /// that omit it) deserialize to the fully-materialized common case.
    #[serde(default = "default_materialized")]
    pub materialized: bool,

    /// Required for committed outcomes; absent for aborted outcomes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<VisibilityState>,
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
            error: None,
            snapshot_version: Some(snapshot_version),
            commit_version: Some(commit_version),
            materialized,
            visibility: Some(VisibilityState::Visible),
        }
    }

    pub fn committed_with_error(
        tx_id: u64,
        snapshot_version: u64,
        commit_version: u64,
        materialized: bool,
        error: impl Into<String>,
    ) -> Self {
        let mut info = Self::committed(tx_id, snapshot_version, commit_version, materialized);
        info.error = Some(error.into());
        info
    }

    pub fn aborted(tx_id: u64, reason: impl Into<String>) -> Self {
        Self {
            tx_id,
            status: "aborted".into(),
            reason: Some(reason.into()),
            error: None,
            snapshot_version: None,
            commit_version: None,
            // Aborted txs never materialize; default is irrelevant to the
            // client because `status` already disambiguates.
            materialized: true,
            visibility: None,
        }
    }

    /// An indeterminate outcome reports the ATTEMPTED commit version without
    /// claiming success — the `indeterminate` status and the
    /// `wal_outcome_unknown` error carry the uncertainty; WAL replay after
    /// reopen decides (CRC-valid frame = committed at this version;
    /// missing/torn = never written).
    pub fn indeterminate(tx_id: u64, snapshot_version: u64, commit_version: u64) -> Self {
        Self {
            tx_id,
            status: "indeterminate".into(),
            reason: None,
            error: Some("wal_outcome_unknown".into()),
            snapshot_version: Some(snapshot_version),
            commit_version: Some(commit_version),
            materialized: false,
            visibility: None,
        }
    }

    /// Set the orthogonal visibility observation without changing commit status.
    pub fn with_visibility(mut self, visibility: VisibilityState) -> Self {
        self.visibility = Some(visibility);
        self
    }

    pub fn is_visible(&self) -> bool {
        self.is_committed() && self.visibility == Some(VisibilityState::Visible)
    }

    /// Validate the v3 metadata contract, including sampled-prefix arithmetic.
    pub fn validate(&self) -> Result<(), &'static str> {
        match (self.status.as_str(), self.visibility) {
            ("committed", Some(visibility)) if self.reason.is_none() => visibility.validate(),
            ("aborted", None) => Ok(()),
            // Visibility absent, reason absent, attempted version required.
            ("indeterminate", None) if self.reason.is_none() && self.commit_version.is_some() => {
                Ok(())
            }
            _ => Err("malformed transaction visibility metadata"),
        }
    }

    pub fn is_committed(&self) -> bool {
        self.status == "committed"
    }

    pub fn is_indeterminate(&self) -> bool {
        self.status == "indeterminate"
    }
}

impl<'de> Deserialize<'de> for TransactionInfo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Decode named fields before enforcing the cross-field v3 invariant.
        #[derive(Deserialize)]
        struct Fields {
            tx_id: u64,
            status: String,
            reason: Option<String>,
            error: Option<String>,
            snapshot_version: Option<u64>,
            commit_version: Option<u64>,
            #[serde(default = "default_materialized")]
            materialized: bool,
            visibility: Option<VisibilityState>,
        }
        let fields = Fields::deserialize(deserializer)?;
        let info = Self {
            tx_id: fields.tx_id,
            status: fields.status,
            reason: fields.reason,
            error: fields.error,
            snapshot_version: fields.snapshot_version,
            commit_version: fields.commit_version,
            materialized: fields.materialized,
            visibility: fields.visibility,
        };
        info.validate().map_err(serde::de::Error::custom)?;
        Ok(info)
    }
}
