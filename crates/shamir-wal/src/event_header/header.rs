//! The event header types embedded in WAL entry format version 3.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_types::access::Actor;

/// Everything a journal event needs that the WAL entry's data ops alone
/// do not carry. Present (`Some`) only in format version 3 entries;
/// versions 1-2 decode with `event: None` ("unknown" — conservative GAP
/// fallback at recovery).
///
/// Allocations: built once per commit at entry construction. The strings
/// clone data the commit already holds (the repo name and the tx's
/// `table_tokens`); `extra_changes` is empty (no allocation) for the
/// common all-`RecordId` commit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WalEventHeader {
    /// Repository name this commit belongs to (the event's `repo` field).
    pub repo: String,
    /// The actor that initiated the transaction.
    pub actor: Actor,
    /// The PUBLIC transaction id: the internal `tx.tx_id`, or `0` for an
    /// implicit batch write ("0 = non-tx write" contract,
    /// `docs/dev-artifacts/roadmap/LIVE_SUBSCRIPTIONS.md`).
    pub tx_id: u64,
    /// Wall-clock nanoseconds sampled at entry construction (best-effort
    /// observability; `commit_version` orders events, never this field).
    pub timestamp_ns: u64,
    /// Human-readable name for every table token this commit staged data
    /// writes for. Tokens absent here resolve to `token:{t}` at derivation
    /// — exactly the fallback the live projection used.
    pub table_names: Vec<(u64, String)>,
    /// Record changes whose staged key is NOT a 16-byte `RecordId` (the
    /// `Put`/`Delete` data ops cover only `RecordId`-keyed writes while
    /// the journal projection visits every `Set`/`Remove`). `change_index`
    /// is the op's 0-based position among ALL of this commit's projected
    /// changes (in `write_set` iteration order), so derivation reproduces
    /// the projected change order exactly. Usually empty.
    pub extra_changes: Vec<WalExtraChange>,
}

/// One non-`RecordId`-keyed record change carried in the event header.
/// `value` is `Some` for a put (FULL new record bytes) and `None` for a
/// delete — the same payload shape the journal event's `RecordChange`
/// uses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WalExtraChange {
    /// Interned table token this change belongs to.
    pub table_id_interned: u64,
    /// 0-based position of this change among ALL of this commit's
    /// projected changes (see [`WalEventHeader::extra_changes`]).
    pub change_index: u32,
    /// Raw staged storage key (NOT a 16-byte `RecordId`).
    #[serde(with = "serde_bytes_bytes")]
    pub key: Bytes,
    /// New record bytes for a put; `None` for a delete.
    #[serde(with = "serde_opt_bytes")]
    pub value: Option<Bytes>,
}

mod serde_bytes_bytes {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        serde_bytes::Bytes::new(b.as_ref()).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        let bb = serde_bytes::ByteBuf::deserialize(d)?;
        Ok(Bytes::from(bb.into_vec()))
    }
}

mod serde_opt_bytes {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &Option<Bytes>, s: S) -> Result<S::Ok, S::Error> {
        match b {
            Some(b) => s.serialize_some(b.as_ref()),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Bytes>, D::Error> {
        let v = Option::<serde_bytes::ByteBuf>::deserialize(d)?;
        Ok(v.map(|bb| Bytes::from(bb.into_vec())))
    }
}
