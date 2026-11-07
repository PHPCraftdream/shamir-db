//! Tolerant wire decoding of one NESTED `TransactionInfo`.
//!
//! The server serializes nested results through `QueryValueSerializer`,
//! which turns every u64 above `i64::MAX` into `QueryValue::Big` — a
//! decimal string on the wire. The typed `TransactionInfo` (plain u64
//! fields) would reject those strings, so nested receipts decode through
//! this equivalent DTO first and are then mapped onto the typed struct;
//! the strict v3 validation still runs downstream
//! (`CommitResult::from_transaction`).

use serde::Deserialize;
use shamir_query_types::batch::{TransactionInfo, VisibilityPendingReason, VisibilityState};
use shamir_types::types::value::QueryValue;

use crate::ClientError;

/// u64 accepting msgpack uint/int or the decimal string of `QueryValue::Big`.
mod u64_flex {
    use serde::de::{Deserializer, Error as DeError, Visitor};
    use std::fmt;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = u64;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("u64 as msgpack uint/int or a decimal string")
            }
            fn visit_u64<E: DeError>(self, v: u64) -> Result<u64, E> {
                Ok(v)
            }
            fn visit_i64<E: DeError>(self, v: i64) -> Result<u64, E> {
                u64::try_from(v).map_err(DeError::custom)
            }
            fn visit_str<E: DeError>(self, v: &str) -> Result<u64, E> {
                v.parse::<u64>().map_err(DeError::custom)
            }
        }
        d.deserialize_any(V)
    }

    pub fn deserialize_option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        struct OV;
        impl<'de> Visitor<'de> for OV {
            type Value = Option<u64>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an optional u64 position")
            }
            fn visit_none<E: DeError>(self) -> Result<Option<u64>, E> {
                Ok(None)
            }
            fn visit_unit<E: DeError>(self) -> Result<Option<u64>, E> {
                Ok(None)
            }
            fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Option<u64>, D2::Error> {
                deserialize(d).map(Some)
            }
        }
        d.deserialize_option(OV)
    }
}

/// Visibility mirror tolerant to `Big` decimal-string positions.
#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum NestedVisibilityWire {
    Visible,
    Pending {
        #[serde(deserialize_with = "u64_flex::deserialize")]
        visible_through: u64,
        #[serde(deserialize_with = "u64_flex::deserialize")]
        blocking_version: u64,
        reason: VisibilityPendingReason,
    },
}

/// TransactionInfo mirror tolerant to `Big` decimal-string positions.
#[derive(Deserialize)]
struct NestedTransactionWire {
    #[serde(deserialize_with = "u64_flex::deserialize")]
    tx_id: u64,
    status: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, deserialize_with = "u64_flex::deserialize_option")]
    snapshot_version: Option<u64>,
    #[serde(default, deserialize_with = "u64_flex::deserialize_option")]
    commit_version: Option<u64>,
    #[serde(default = "default_true")]
    materialized: bool,
    visibility: Option<NestedVisibilityWire>,
}

fn default_true() -> bool {
    true
}

/// Reverse of the server's msgpack round-trip for one nested
/// `TransactionInfo`, tolerant to `Big` decimal-string positions.
pub(crate) fn decode_nested_transaction(
    receipt: &QueryValue,
) -> Result<TransactionInfo, ClientError> {
    let bytes = rmp_serde::to_vec_named(receipt)
        .map_err(|e| ClientError::Protocol(format!("nested receipt encode: {e}")))?;
    let wire: NestedTransactionWire = rmp_serde::from_slice(&bytes)
        .map_err(|e| ClientError::Protocol(format!("nested receipt: {e}")))?;
    Ok(TransactionInfo {
        tx_id: wire.tx_id,
        status: wire.status,
        reason: wire.reason,
        error: wire.error,
        snapshot_version: wire.snapshot_version,
        commit_version: wire.commit_version,
        materialized: wire.materialized,
        visibility: wire.visibility.map(|v| match v {
            NestedVisibilityWire::Visible => VisibilityState::Visible,
            NestedVisibilityWire::Pending {
                visible_through,
                blocking_version,
                reason,
            } => VisibilityState::Pending {
                visible_through,
                blocking_version,
                reason,
            },
        }),
    })
}
