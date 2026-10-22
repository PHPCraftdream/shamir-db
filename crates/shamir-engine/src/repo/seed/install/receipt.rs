use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};

use crate::meta::recovery_marker::{load_last_committed, load_next_tx_id_snapshot};
use crate::meta::repl_bookmark::load_replication_bookmark;
use crate::meta::repo_identity::RepoIdentity;
use crate::meta::MetaEnvelope;

const RECEIPT_KEY: &[u8] = b"__meta__/seed_install/receipt";

/// Source evidence is deliberately separate from the destination RepoIdentity.
/// Digests are public integrity identifiers, not authentication credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedReceipt {
    pub artifact_digest: String,
    pub source_db: String,
    pub source_repo: String,
    pub source_identity: RepoIdentity,
    pub schema_digest: String,
    pub source_version: u64,
    pub local_baseline: u64,
}

impl SeedReceipt {
    pub fn validate(&self) -> DbResult<()> {
        let digest = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        // Epoch zero is a fresh repository incarnation, not a reserved commit version.
        if !digest(&self.artifact_digest)
            || !digest(&self.schema_digest)
            || self.source_db.is_empty()
            || self.source_repo.is_empty()
            || self.source_db.len() > 4096
            || self.source_repo.len() > 4096
            || self.source_version == 0
            || self.source_version == u64::MAX
            || self.local_baseline != 1
        {
            return Err(DbError::Codec("invalid seed receipt".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SeedInstallState {
    Installing(SeedReceipt),
    Complete(SeedReceipt),
}

impl SeedInstallState {
    pub async fn load(store: &Arc<dyn Store>) -> DbResult<Option<Self>> {
        match store.get(RecordKey::from_slice(RECEIPT_KEY)).await {
            Ok(bytes) => {
                if bytes.len() > 32 * 1024 {
                    return Err(DbError::Codec(
                        "seed receipt exceeds metadata budget".into(),
                    ));
                }
                let state: Self = MetaEnvelope::open(&bytes)
                    .map_err(|e| DbError::Codec(format!("seed receipt: {e}")))?;
                match &state {
                    Self::Installing(receipt) | Self::Complete(receipt) => receipt.validate()?,
                }
                Ok(Some(state))
            }
            Err(DbError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) async fn save(&self, store: &Arc<dyn Store>) -> DbResult<()> {
        let bytes = MetaEnvelope::new(self.clone())
            .encode()
            .map_err(|e| DbError::Codec(format!("seed receipt: {e}")))?;
        store
            .set(RecordKey::from_slice(RECEIPT_KEY), Bytes::from(bytes))
            .await?;
        store.flush().await
    }

    /// Pre-registry gate. The catalogue/staging owner supplies designation;
    /// an ordinary repository with no seed state remains admissible.
    pub async fn require_complete(
        store: &Arc<dyn Store>,
        install_designated: bool,
    ) -> DbResult<Option<SeedReceipt>> {
        match Self::load(store).await? {
            Some(Self::Complete(receipt)) => {
                Self::validate_progress(store, &receipt).await?;
                Ok(Some(receipt))
            }
            Some(Self::Installing(_)) => {
                Err(DbError::Storage("seed installation is incomplete".into()))
            }
            None if install_designated => Err(DbError::Storage(
                "designated seed image has no Complete receipt".into(),
            )),
            None => Ok(None),
        }
    }

    async fn validate_progress(store: &Arc<dyn Store>, receipt: &SeedReceipt) -> DbResult<()> {
        if load_last_committed(store).await?.unwrap_or(0) < 1
            || load_next_tx_id_snapshot(store).await?.unwrap_or(0) < 1
            || load_replication_bookmark(store).await? < receipt.source_version
        {
            return Err(DbError::Storage(
                "Complete seed has inconsistent recovery floors".into(),
            ));
        }
        Ok(())
    }

    /// Exact retries finish publication, never replay rows into this image.
    pub async fn completed_retry(store: &Arc<dyn Store>, expected: &SeedReceipt) -> DbResult<bool> {
        expected.validate()?;
        match Self::load(store).await? {
            Some(Self::Complete(actual)) if actual == *expected => {
                Self::validate_progress(store, expected).await?;
                // A prior failed flush can leave Complete in the live view.
                store.flush().await?;
                Ok(true)
            }
            Some(Self::Complete(_)) => Err(DbError::Storage("conflicting seed receipt".into())),
            Some(Self::Installing(_)) => Err(DbError::Storage(
                "incomplete seed requires fresh staging".into(),
            )),
            None => Ok(false),
        }
    }
}
