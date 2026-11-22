//! Checkpoint-row storage for the raw appender surface: strict decode,
//! anchor validation and the durable single-row write.
//!
//! Round-3 item 4 (raw parity with boot recovery): a stored checkpoint row
//! that is malformed (wrong byte length), a zero next sequence or an
//! invalid genesis tuple is a typed [`AppenderError`] — never a silent
//! `None` and never silently overwritable. `recovery.rs` keeps its own
//! decoder for the boot path; [`validate`] is the
//! raw-side rule mirroring recovery's trusted-anchor check, and
//! [`FjallAuditAppender::write_checkpoint`] refuses an invalid attempted tuple even on a fresh
//! store, before anything is inserted.

use super::{AppenderError, FjallAuditAppender, Position, CHECKPOINT_KEY, CHECKPOINT_KEYSPACE};
use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use shamir_connect::common::crypto::constant_time_eq;
use std::path::Path;

/// Genesis chain position (`recovery::GENESIS` twin): next sequence 1 with
/// the all-zero digest.
const GENESIS: Position = (1, [0; 32]);

/// The trusted-anchor tuple rule shared with boot recovery (one
/// definition): the next sequence must be non-zero, and at the genesis
/// position the digest must be the genesis digest. Pure predicate — callers
/// map it to their own typed error (raw: [`AppenderError::InvalidAnchor`];
/// recovery: `AuditRecoveryError::InvalidAnchor`).
pub(super) fn is_valid_anchor(next_seq: u64, digest: &[u8; 32]) -> bool {
    !(next_seq == 0 || (next_seq == GENESIS.0 && !constant_time_eq(digest, &GENESIS.1)))
}

/// [`is_valid_anchor`] as a typed raw-surface refusal.
fn validate(next_seq: u64, prev_hmac: &[u8; 32]) -> Result<(), AppenderError> {
    if is_valid_anchor(next_seq, prev_hmac) {
        Ok(())
    } else {
        Err(AppenderError::InvalidAnchor { next_seq })
    }
}

/// Strictly decode a checkpoint row: exactly 40 bytes — an 8-byte BE next
/// sequence plus a 32-byte digest — forming a valid anchor tuple. Bad bytes
/// are a typed error, never `None`; the raw bytes stay on disk for
/// diagnosis.
fn decode_strict(bytes: &[u8]) -> Result<Position, AppenderError> {
    let malformed = || AppenderError::MalformedCheckpoint {
        length: bytes.len(),
    };
    let Some((seq, rest)) = bytes.split_first_chunk::<8>() else {
        return Err(malformed());
    };
    let Ok(hmac) = <[u8; 32]>::try_from(rest) else {
        return Err(malformed());
    };
    let position = (u64::from_be_bytes(*seq), hmac);
    validate(position.0, &position.1)?;
    Ok(position)
}

impl FjallAuditAppender {
    /// Persist checkpoint metadata. Caller owns the persistence gate.
    ///
    /// Monotonicity is checked against the ACTUAL persisted anchor row
    /// (`stored_position`), never the fsynced position, so a legacy opener
    /// facing a newer on-disk anchor cannot lower it at rotation or final
    /// close. Refuses regressions / same-position digest conflicts
    /// ([`AppenderError::AnchorRegression`] / [`AppenderError::AnchorConflict`],
    /// digest compared through `constant_time_eq`), an invalid attempted tuple
    /// even on a fresh store ([`AppenderError::InvalidAnchor`]), and a
    /// malformed existing row ([`AppenderError::MalformedCheckpoint`]). Never
    /// touches `latest_fsynced` — that field tracks observed log fsyncs only.
    fn write_checkpoint(&self, next_seq: u64, prev_hmac: &[u8; 32]) -> Result<(), AppenderError> {
        // Reject an invalid attempted tuple BEFORE any comparison with the
        // stored row: a fresh store must not accept an invalid genesis anchor
        // either (round-3 item 4).
        validate(next_seq, prev_hmac)?;
        if let Some(stored) = Self::stored_position(&self.checkpoint_ks)? {
            if next_seq < stored.0 {
                return Err(AppenderError::AnchorRegression {
                    persisted: stored.0,
                    attempted: next_seq,
                });
            }
            if next_seq == stored.0 && !constant_time_eq(&stored.1, prev_hmac) {
                return Err(AppenderError::AnchorConflict { next_seq });
            }
        }
        let mut packed = [0u8; 40];
        packed[..8].copy_from_slice(&next_seq.to_be_bytes());
        packed[8..].copy_from_slice(prev_hmac);
        self.checkpoint_ks.insert(CHECKPOINT_KEY, &packed[..])?;
        self.checkpoint_db.persist(PersistMode::SyncAll)?;
        Ok(())
    }

    /// Persist checkpoint while holding the same gate as every log drain.
    pub(super) fn checkpoint_locked(
        &self,
        next_seq: u64,
        prev_hmac: &[u8; 32],
    ) -> Result<(), AppenderError> {
        self.flush_buffer_locked()?;
        self.write_checkpoint(next_seq, prev_hmac)
    }

    /// Persist the fsynced anchor and run the optional server-meta hook
    /// (rotation boundaries and final shutdown). Caller owns the gate.
    pub(super) fn persist_anchor_and_meta(&self) -> Result<(), AppenderError> {
        // No fsynced position observed (raw empty close): write nothing —
        // a fresh deployment must not invent a genesis anchor here.
        let Some(position) = *self.latest_fsynced.lock() else {
            return Ok(());
        };
        self.write_checkpoint(position.0, &position.1)?;
        if let Some(hook) = &self.meta_hook {
            hook(position.0, position.1).map_err(AppenderError::Io)?;
        }
        Ok(())
    }

    /// Durable anchor row persisted in the opened checkpoint store, if any.
    /// A malformed or invalid stored row is a typed error (never `None`),
    /// so a raw write can never overwrite it.
    fn stored_position(ks: &Keyspace) -> Result<Option<Position>, AppenderError> {
        let Some(value) = ks.get(CHECKPOINT_KEY)? else {
            return Ok(None);
        };
        decode_strict(value.as_ref()).map(Some)
    }

    /// Read the persisted checkpoint, if any.
    ///
    /// Returns `Ok(None)` for fresh deployments (no store or empty
    /// keyspace). A malformed row (wrong length) or an invalid anchor tuple
    /// (zero next sequence, or genesis position with a non-genesis digest)
    /// is a typed error preserving the bad bytes for diagnosis — round-3
    /// item 4: parity with what recovery refuses at boot.
    pub fn load_checkpoint(
        data_dir: impl AsRef<Path>,
    ) -> Result<Option<(u64, [u8; 32])>, AppenderError> {
        let (_log_path, db_path) = Self::paths(data_dir.as_ref());
        if !db_path.exists() {
            return Ok(None);
        }
        let db = Database::builder(&db_path).open()?;
        let ks = db.keyspace(CHECKPOINT_KEYSPACE, KeyspaceCreateOptions::default)?;
        Self::stored_position(&ks)
    }
}
