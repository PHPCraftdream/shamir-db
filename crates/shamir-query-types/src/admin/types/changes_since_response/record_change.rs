//! [`ChangesSinceRecordChange`] — client-side mirror of one record-level
//! change inside a `changes_since` journal event.

use serde::{Deserialize, Serialize};

/// One record-level change inside a
/// [`ChangesSinceEvent`](super::event::ChangesSinceEvent).
///
/// Faithful wire mirror of `shamir_tx::changefeed::RecordChange` (the
/// crate cannot depend on `shamir-tx`). Its adapters are byte-for-byte the
/// server's own (`serde_bytes_compat` / `serde_opt_bytes_compat`), so on
/// the wire `key` is MessagePack BIN (`serialize_bytes`) while the
/// optional `value` is an ARRAY OF BYTE INTEGERS: the server's
/// `serde_opt_bytes_compat` calls `serialize_some(v.as_ref())`, rmp-serde
/// serializes `Some` transparently and the `&[u8]` slice goes through
/// `collect_seq` with `BytesMode::Normal` (the default), producing a
/// fixarray — NOT bin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesSinceRecordChange {
    /// Human-readable table name the change belongs to.
    pub table: String,
    /// Raw storage key (16-byte `RecordId` for data tables).
    #[serde(with = "bytes_as_bin")]
    pub key: Vec<u8>,
    /// The mutation kind.
    pub op: ChangeOpKind,
    /// New record bytes for [`ChangeOpKind::Put`]; `None` for
    /// [`ChangeOpKind::Delete`].
    #[serde(with = "opt_bytes_as_bin")]
    pub value: Option<Vec<u8>>,
}

/// The mutation kind of a [`ChangesSinceRecordChange`]. Mirrors
/// `shamir_tx::changefeed::ChangeOp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeOpKind {
    /// Insert or update — `value` carries the new bytes.
    Put,
    /// Delete — `value` is `None`.
    Delete,
}

/// `Vec<u8>` ↔ MessagePack bin adapter (mirrors the server-side
/// `serde_bytes_compat` in `shamir-tx::changefeed`).
mod bytes_as_bin {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        Vec::<u8>::deserialize(d)
    }
}

/// `Option<Vec<u8>>` ↔ MessagePack bin adapter (mirrors the server-side
/// `serde_opt_bytes_compat` in `shamir-tx::changefeed`).
mod opt_bytes_as_bin {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.serialize_some(v.as_slice()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<Vec<u8>>::deserialize(d)
    }
}
