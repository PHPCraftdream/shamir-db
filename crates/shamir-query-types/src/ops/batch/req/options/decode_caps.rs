//! Decode-time entry caps for client-keyed collections in
//! [`BatchRequest`](crate::batch::batch_request::BatchRequest).
//!
//! Alias-keyed collections are capped at [`BatchLimits::CEILING`]`
//! .max_queries` (1000) DURING deserialization: the map/seq visitor stops at
//! entry 1001, so an over-cap payload fails as a serde error (surfaced by the
//! server as wire code `invalid_request`) instead of materializing an
//! unbounded `TMap`. Applies recursively: nested `SubBatchOp.batch` decodes
//! through the same `Deserialize` impl of `BatchRequest`.
//!
//! Nested-batch note: `BatchOp::deserialize` buffers the entire nested op into
//! a `QueryValue` before typed `SubBatchOp` decoding. Duplicate keys in that
//! intermediate map can collapse, so a typed `BatchRequest` decode (including
//! `decode_caps`'s capped visitors) is not a trustworthy network boundary.
//! Untrusted request bytes MUST pass `validate_request_alias_caps` first; its
//! raw-wire walk checks nested maps before buffering or duplicate collapse.
//! [`assert_nested_batch_caps`] is a typed-value helper, not the network
//! preflight guarantee.

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::fmt;
use std::marker::PhantomData;

use shamir_collections::TMap;

use crate::batch::batch_limits::BatchLimits;

/// Maximum number of entries in any client-keyed alias collection.
pub(crate) const MAX_ALIAS_ENTRIES: usize = BatchLimits::CEILING.max_queries;

/// Deserialize a `TMap<String, V>` with at most [`MAX_ALIAS_ENTRIES`] entries.
pub(crate) fn capped_string_map<'de, D, V>(deserializer: D) -> Result<TMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct CappedMapVisitor<V>(PhantomData<V>);

    impl<'de, V: Deserialize<'de>> Visitor<'de> for CappedMapVisitor<V> {
        type Value = TMap<String, V>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "a map of at most {} entries", MAX_ALIAS_ENTRIES)
        }

        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut map = shamir_collections::new_map();
            // Count WIRE entries, not distinct keys: duplicate keys must
            // also grow the counter or 1001 repeated keys slip past.
            let mut visited = 0usize;
            while let Some(key) = access.next_key::<String>()? {
                visited += 1;
                if visited > MAX_ALIAS_ENTRIES {
                    return Err(de::Error::custom(cap_error_message("map")));
                }
                let value = access.next_value::<V>()?;
                map.insert(key, value);
            }
            Ok(map)
        }
    }

    deserializer.deserialize_map(CappedMapVisitor(PhantomData))
}

/// Deserialize an `Option<Vec<String>>` with at most
/// [`MAX_ALIAS_ENTRIES`] entries.
pub(crate) fn capped_string_vec_opt<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    // Option-level visitor (mirrors serde's OptionVisitor): nil → None,
    // Some(seq) → collect through the bounded SeqCollector.
    struct CappedSeqVisitor;

    impl<'de> Visitor<'de> for CappedSeqVisitor {
        type Value = Option<Vec<String>>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "an optional sequence of at most {} entries",
                MAX_ALIAS_ENTRIES
            )
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_seq(SeqCollector).map(Some)
        }
    }

    struct SeqCollector;

    impl<'de> Visitor<'de> for SeqCollector {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "a sequence of at most {} entries", MAX_ALIAS_ENTRIES)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut vec = Vec::new();
            let mut visited = 0usize;
            while let Some(item) = access.next_element::<String>()? {
                visited += 1;
                if visited > MAX_ALIAS_ENTRIES {
                    return Err(de::Error::custom(cap_error_message("sequence")));
                }
                vec.push(item);
            }
            Ok(vec)
        }
    }

    deserializer.deserialize_option(CappedSeqVisitor)
}

fn cap_error_message(kind: &str) -> String {
    format!("{kind} exceeds the decode cap of {MAX_ALIAS_ENTRIES} alias entries")
}

/// Enforce the alias-entry caps on an ALREADY-BUFFERED nested batch value.
///
/// `BatchOp::deserialize` materializes the whole op into a `QueryValue`
/// before dispatching, so the typed `SubBatchOp`/`BatchRequest` decode (and
/// its cap visitors) only run after full buffering + re-encode. This walk
/// closes that window: it recurses along `batch` keys only and checks the
/// `queries` / `interner_epochs` map lens and `return_only` list lens
/// against [`MAX_ALIAS_ENTRIES`], so an over-cap nested payload fails with
/// a cap error BEFORE the re-encode and typed decode. Buffering itself is
/// `BatchOp`'s pre-existing design and is not changed here.
pub(crate) fn assert_nested_batch_caps(
    qv: &shamir_types::types::value::QueryValue,
) -> Result<(), String> {
    match qv {
        shamir_types::types::value::QueryValue::Map(map) => {
            if let Some(batch) = map.get("batch") {
                check_batch_fields(batch)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Inspect one nested batch value (the `"batch"` entry): check its capped
/// fields, then recurse into every per-query op value for deeper nesting.
fn check_batch_fields(batch: &shamir_types::types::value::QueryValue) -> Result<(), String> {
    let shamir_types::types::value::QueryValue::Map(batch_map) = batch else {
        return Ok(());
    };
    for (field, value) in [
        ("queries", batch_map.get("queries")),
        ("interner_epochs", batch_map.get("interner_epochs")),
    ] {
        if let Some(shamir_types::types::value::QueryValue::Map(m)) = value {
            if m.len() > MAX_ALIAS_ENTRIES {
                return Err(cap_error_message(field));
            }
        }
    }
    if let Some(shamir_types::types::value::QueryValue::List(items)) = batch_map.get("return_only")
    {
        if items.len() > MAX_ALIAS_ENTRIES {
            return Err(cap_error_message("return_only"));
        }
    }
    if let Some(shamir_types::types::value::QueryValue::Map(m)) = batch_map.get("queries") {
        for (_alias, op) in m.iter() {
            assert_nested_batch_caps(op)?;
        }
    }
    Ok(())
}
