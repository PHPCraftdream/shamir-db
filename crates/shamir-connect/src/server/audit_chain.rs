//! HMAC-chained audit log per spec IMPL §3.3 NORMATIVE.
//!
//! Append-only structured event log where each entry's HMAC links to the
//! previous one's HMAC. Tampering with ANY entry breaks the chain and is
//! detected on verification (typically at startup).
//!
//! ## Chain construction
//!
//! ```text
//! entry.hmac = HMAC-SHA256(
//!     audit_chain_key,
//!     prev_hmac || canonical_bytes(entry_without_hmac)
//! )
//! ```
//!
//! First entry uses `prev_hmac = bytes(32) zeros`, `seq = 1`. Each
//! subsequent entry sets `prev_hmac = previous_entry.hmac` and
//! `seq = previous_entry.seq + 1`.
//!
//! ## canonical_bytes layout (per spec §3.3)
//!
//! ```text
//! u64_be(seq)
//! u64_be(ts_ns)
//! u16_be(byte_len(event)) || event_utf8
//! u8(byte_len(transport))  || transport_utf8
//! u8(byte_len(user))       || user_utf8        // empty if null
//! u8(byte_len(ip_subnet))  || ip_subnet_utf8
//! bytes(8) session_id_prefix                    // zeros if null
//! u8(byte_len(result))     || result_utf8
//! u32_be(byte_len(details_msgpack)) || details_canonical_msgpack
//! prev_hmac(32)
//! ```
//!
//! ## Truncation defence
//!
//! `last_audit_hmac` checkpoint persisted to `server_meta` periodically
//! (every 60s OR every 1000 events, whichever first). On startup the
//! verifier walks the log and asserts the final `hmac` matches
//! `last_audit_hmac`. A truncation attack (delete trailing entries)
//! produces a different final hmac → `audit_chain_verify_failed` event.

use crate::common::crypto::{constant_time_eq, hmac_sha256};
use parking_lot::Mutex;
use std::sync::Arc;

/// One audit log entry per spec §3.3.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Sequence number — starts at 1, monotonically increases per chain.
    pub seq: u64,
    /// Wall-clock timestamp (unix nanos).
    pub ts_ns: u64,
    /// Event name — see spec §3.2 for the enum (e.g. "auth_success",
    /// "auth_failed", "session_evicted", "rotateServerIdentity", etc.).
    pub event: String,
    /// Transport identifier (e.g. "tcp", "wss", "wss-browser").
    pub transport: String,
    /// Username (post-PRECIS) or empty string if not applicable.
    pub user: String,
    /// IP subnet of the connecting client (e.g. "192.0.2.0/24").
    pub ip_subnet: String,
    /// First 8 bytes of session_id (or zeros for non-session events).
    pub session_id_prefix: [u8; 8],
    /// Outcome: "ok", "failed", "rate_limited", "lockout", etc.
    pub result: String,
    /// Event-specific details — canonical msgpack-encoded (caller
    /// supplies bytes; see [`encode_details`] for a default map encoder).
    pub details_canonical_msgpack: Vec<u8>,
    /// HMAC of the previous entry (32 zeros for the first entry).
    pub prev_hmac: [u8; 32],
    /// HMAC of this entry — populated by [`AuditChain::append`].
    pub hmac: [u8; 32],
}

/// Build the canonical byte-string of an entry (excluding `hmac`).
///
/// Per spec §3.3 — deterministic concatenation that any conforming
/// implementation (Rust, JS, Python) must reproduce byte-identically.
pub fn canonical_bytes(entry: &AuditEntry) -> Vec<u8> {
    let user_bytes = entry.user.as_bytes();
    let event_bytes = entry.event.as_bytes();
    let transport_bytes = entry.transport.as_bytes();
    let ip_bytes = entry.ip_subnet.as_bytes();
    let result_bytes = entry.result.as_bytes();
    let details = &entry.details_canonical_msgpack;

    let cap = 8                                   // seq
        + 8                                       // ts_ns
        + 2 + event_bytes.len()
        + 1 + transport_bytes.len()
        + 1 + user_bytes.len()
        + 1 + ip_bytes.len()
        + 8                                       // session_id_prefix
        + 1 + result_bytes.len()
        + 4 + details.len()
        + 32; // prev_hmac

    let mut out = Vec::with_capacity(cap);
    out.extend_from_slice(&entry.seq.to_be_bytes());
    out.extend_from_slice(&entry.ts_ns.to_be_bytes());
    out.extend_from_slice(&(event_bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(event_bytes);
    out.push(transport_bytes.len() as u8);
    out.extend_from_slice(transport_bytes);
    out.push(user_bytes.len() as u8);
    out.extend_from_slice(user_bytes);
    out.push(ip_bytes.len() as u8);
    out.extend_from_slice(ip_bytes);
    out.extend_from_slice(&entry.session_id_prefix);
    out.push(result_bytes.len() as u8);
    out.extend_from_slice(result_bytes);
    out.extend_from_slice(&(details.len() as u32).to_be_bytes());
    out.extend_from_slice(details);
    out.extend_from_slice(&entry.prev_hmac);
    debug_assert_eq!(out.len(), cap);
    out
}

/// Compute the HMAC for an entry given the chain key.
pub fn compute_entry_hmac(audit_chain_key: &[u8; 32], entry: &AuditEntry) -> [u8; 32] {
    hmac_sha256(audit_chain_key, &canonical_bytes(entry))
}

/// HMAC chain state — append-only writer + tamper-detection verifier.
///
/// Single-mutex inner state holds the next `seq`, the `prev_hmac`, and
/// (optionally) a list of entries for in-memory deployments.
pub struct AuditChain {
    audit_chain_key: [u8; 32],
    inner: Mutex<ChainInner>,
}

struct ChainInner {
    next_seq: u64,
    prev_hmac: [u8; 32],
    /// Entries accumulated in-memory (for tests + simple deployments).
    /// Production should override with a streaming writer that persists
    /// each entry to durable log + bumps `last_audit_hmac` checkpoint.
    entries: Vec<AuditEntry>,
}

impl core::fmt::Debug for AuditChain {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let g = self.inner.lock();
        f.debug_struct("AuditChain")
            .field("audit_chain_key", &"<REDACTED:32>")
            .field("next_seq", &g.next_seq)
            .field("prev_hmac", &"<REDACTED:32>")
            .field("entries_in_memory", &g.entries.len())
            .finish()
    }
}

impl AuditChain {
    /// Fresh chain — `prev_hmac = zeros`, `seq` starts at 1.
    pub fn new(audit_chain_key: [u8; 32]) -> Self {
        Self {
            audit_chain_key,
            inner: Mutex::new(ChainInner {
                next_seq: 1,
                prev_hmac: [0u8; 32],
                entries: Vec::new(),
            }),
        }
    }

    /// Construct from a checkpoint (e.g. after restart): the next entry
    /// will have `seq = next_seq` and chain from `prev_hmac`.
    pub fn from_checkpoint(audit_chain_key: [u8; 32], next_seq: u64, prev_hmac: [u8; 32]) -> Self {
        Self {
            audit_chain_key,
            inner: Mutex::new(ChainInner {
                next_seq,
                prev_hmac,
                entries: Vec::new(),
            }),
        }
    }

    /// Append an event. Caller supplies the entry fields; this method
    /// fills in `seq`, `prev_hmac`, computes `hmac`, and updates state.
    /// Returns the persisted [`AuditEntry`].
    #[allow(clippy::too_many_arguments)] // audit fields are per-spec, not groupable
    pub fn append(
        &self,
        event: impl Into<String>,
        transport: impl Into<String>,
        user: impl Into<String>,
        ip_subnet: impl Into<String>,
        session_id_prefix: [u8; 8],
        result: impl Into<String>,
        details_canonical_msgpack: Vec<u8>,
        ts_ns: u64,
    ) -> AuditEntry {
        let mut g = self.inner.lock();
        let mut entry = AuditEntry {
            seq: g.next_seq,
            ts_ns,
            event: event.into(),
            transport: transport.into(),
            user: user.into(),
            ip_subnet: ip_subnet.into(),
            session_id_prefix,
            result: result.into(),
            details_canonical_msgpack,
            prev_hmac: g.prev_hmac,
            hmac: [0u8; 32],
        };
        entry.hmac = compute_entry_hmac(&self.audit_chain_key, &entry);

        g.prev_hmac = entry.hmac;
        g.next_seq = g.next_seq.saturating_add(1);
        g.entries.push(entry.clone());
        entry
    }

    /// Snapshot the current chain checkpoint: `(next_seq, prev_hmac)`.
    /// Persist this to `server_meta.last_audit_hmac` periodically per
    /// spec §3.3 truncation defence.
    pub fn checkpoint(&self) -> (u64, [u8; 32]) {
        let g = self.inner.lock();
        (g.next_seq, g.prev_hmac)
    }

    /// Snapshot all in-memory entries (test helper).
    pub fn snapshot(&self) -> Vec<AuditEntry> {
        self.inner.lock().entries.clone()
    }

    /// Verify a sequence of entries against this chain key. Returns
    /// `Ok(())` if every entry's hmac matches its canonical bytes AND
    /// the chain links are intact.
    ///
    /// Use after loading entries from durable storage at startup.
    pub fn verify_chain(
        audit_chain_key: &[u8; 32],
        entries: &[AuditEntry],
    ) -> Result<(), AuditError> {
        let mut expected_seq = 1u64;
        let mut expected_prev = [0u8; 32];
        for (idx, e) in entries.iter().enumerate() {
            if e.seq != expected_seq {
                return Err(AuditError::SequenceGap {
                    at: idx,
                    expected: expected_seq,
                    found: e.seq,
                });
            }
            if !constant_time_eq(&e.prev_hmac, &expected_prev) {
                return Err(AuditError::ChainBroken { at: idx });
            }
            let computed = compute_entry_hmac(audit_chain_key, e);
            if !constant_time_eq(&e.hmac, &computed) {
                return Err(AuditError::HmacMismatch { at: idx });
            }
            expected_prev = e.hmac;
            expected_seq = expected_seq.saturating_add(1);
        }
        Ok(())
    }

    /// Truncation-defence verifier: after [`Self::verify_chain`] succeeds,
    /// also check that the final entry's hmac matches the persisted
    /// checkpoint. If checkpoint is ahead of the chain → trailing entries
    /// were truncated.
    pub fn verify_against_checkpoint(
        entries: &[AuditEntry],
        checkpoint_seq: u64,
        checkpoint_hmac: &[u8; 32],
    ) -> Result<(), AuditError> {
        let last = entries.last();
        let (last_seq, last_hmac) = match last {
            Some(e) => (e.seq, e.hmac),
            None => (0, [0u8; 32]),
        };
        if checkpoint_seq != last_seq.saturating_add(1) && checkpoint_seq != last_seq {
            return Err(AuditError::TruncationDetected {
                checkpoint_seq,
                final_seq: last_seq,
            });
        }
        if checkpoint_seq == last_seq.saturating_add(1)
            && !constant_time_eq(&last_hmac, checkpoint_hmac)
        {
            return Err(AuditError::TruncationDetected {
                checkpoint_seq,
                final_seq: last_seq,
            });
        }
        Ok(())
    }
}

/// Errors raised by [`AuditChain::verify_chain`] /
/// [`AuditChain::verify_against_checkpoint`].
#[derive(Debug, PartialEq, Eq)]
pub enum AuditError {
    /// Entry's `seq` doesn't match the expected position.
    SequenceGap {
        /// Index of the entry in the verified slice.
        at: usize,
        /// Expected `seq` value.
        expected: u64,
        /// Actual `seq` value found.
        found: u64,
    },
    /// Entry's `prev_hmac` doesn't match the previous entry's `hmac`.
    ChainBroken {
        /// Index where the chain link was found broken.
        at: usize,
    },
    /// Entry's `hmac` doesn't match its canonical bytes.
    HmacMismatch {
        /// Index where the hmac mismatch was found.
        at: usize,
    },
    /// Persisted checkpoint is past the loaded entries — trailing entries
    /// were truncated.
    TruncationDetected {
        /// Persisted `next_seq` from the checkpoint.
        checkpoint_seq: u64,
        /// `seq` of the last loaded entry.
        final_seq: u64,
    },
}

impl core::fmt::Display for AuditError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for AuditError {}

/// Pluggable backend for streaming audit entries to durable storage.
///
/// Production deployments wrap an `AuditChain` + a `AuditAppender` so
/// each `append` also persists the entry's canonical bytes + hmac to
/// sqlite / etc.
pub trait AuditAppender: Send + Sync {
    /// Persist one entry. The implementation is free to batch BUT must
    /// fsync at least every 5 seconds (per spec §3.3 + IMPL §1.3).
    fn append_entry(&self, entry: &AuditEntry);

    /// Persist the (next_seq, hmac) checkpoint for truncation defence.
    /// Should be called at least every 60s OR every 1000 events.
    fn checkpoint(&self, next_seq: u64, prev_hmac: &[u8; 32]);
}

/// Convenience: encode a `BTreeMap<String, msgpack-Value>` as canonical
/// msgpack (lex-sorted keys) for use as `details_canonical_msgpack`.
///
/// Caller can also supply hand-crafted bytes for tighter control.
pub fn encode_details_canonical(
    map: &std::collections::BTreeMap<String, rmp_serde::config::DefaultConfig>,
) -> Vec<u8> {
    // BTreeMap iterates lex-sorted, so msgpack encoding is canonical.
    let _ = map; // placeholder until callers supply real msgpack values
    Vec::new()
}

/// Wrap an `AuditChain` so each `append` ALSO calls the appender.
///
/// The chain is held behind an `Arc` so the writer and the scheduler
/// (which periodically calls `checkpoint_now`) share a single mutable
/// chain state. Without this share both sides would otherwise operate
/// on independent `AuditChain` instances — producing a split-brain
/// where audit events advance one chain while the truncation-defence
/// checkpoint persists the *other* chain's (empty) state. See
/// `audit_writer_and_checkpoint_share_chain_state` for the invariant.
pub struct AuditChainWriter {
    chain: Arc<AuditChain>,
    appender: Arc<dyn AuditAppender>,
}

impl AuditChainWriter {
    /// Construct from an owned chain. Backwards-compatible convenience:
    /// wraps the supplied chain in an `Arc` internally. Callers that need
    /// to share the chain with the scheduler (the production path) MUST
    /// use [`Self::new_with_shared`] instead.
    pub fn new(chain: AuditChain, appender: Arc<dyn AuditAppender>) -> Self {
        Self {
            chain: Arc::new(chain),
            appender,
        }
    }

    /// Construct from a *shared* chain. The same `Arc<AuditChain>` MUST
    /// also be handed to the scheduler so `checkpoint_now` observes the
    /// state mutated by `append`.
    pub fn new_with_shared(chain: Arc<AuditChain>, appender: Arc<dyn AuditAppender>) -> Self {
        Self { chain, appender }
    }

    /// Snapshot of the shared chain handle — useful when the writer
    /// was built with [`Self::new`] but the caller now needs the same
    /// chain reference for an external checkpoint task. Returns a clone
    /// of the internal `Arc`.
    pub fn chain(&self) -> Arc<AuditChain> {
        Arc::clone(&self.chain)
    }

    /// Append + appender.append + appender.checkpoint (every 1000 entries).
    #[allow(clippy::too_many_arguments)]
    pub fn append(
        &self,
        event: impl Into<String>,
        transport: impl Into<String>,
        user: impl Into<String>,
        ip_subnet: impl Into<String>,
        session_id_prefix: [u8; 8],
        result: impl Into<String>,
        details_canonical_msgpack: Vec<u8>,
        ts_ns: u64,
    ) -> AuditEntry {
        let entry = self.chain.append(
            event,
            transport,
            user,
            ip_subnet,
            session_id_prefix,
            result,
            details_canonical_msgpack,
            ts_ns,
        );
        self.appender.append_entry(&entry);
        // Checkpoint every 1000 entries (truncation defence).
        if entry.seq.is_multiple_of(1000) {
            let (next_seq, prev_hmac) = self.chain.checkpoint();
            self.appender.checkpoint(next_seq, &prev_hmac);
        }
        entry
    }

    /// Force checkpoint NOW (caller schedules this every 60s for
    /// tighter truncation-detection bounds).
    pub fn checkpoint_now(&self) {
        let (next_seq, prev_hmac) = self.chain.checkpoint();
        self.appender.checkpoint(next_seq, &prev_hmac);
    }
}

// Tests live in crate::server::tests::audit_chain_tests.
