use super::MaintenanceGuard;
use crate::table::TableManager;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::StreamExt;
use shamir_collections::{TFxSet, THasher};
use shamir_storage::error::{DbError, DbResult};
use shamir_types::types::value::{InnerValue, QueryValue};

const MAX_WORKING_BYTES: usize = 64 * 1024 * 1024;
const MAX_ROW_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONTAINER_ENTRIES: usize = 65_536;
const NODE_WORKING_BYTES: usize = 192;
const MAP_KEY_WORKING_BYTES: usize = 64;
type RawRows<'a> = BoxStream<'a, DbResult<Vec<(Bytes, Bytes)>>>;

/// Single-row pages deliberately bound source buffering independently of row
/// size. Uses existing MVCC snapshots, or the frozen raw store for tables with
/// no MVCC attachment. The raw-store path has no snapshot or tombstone
/// semantics, so it is suitable only under a fully fenced freeze or exclusive
/// offline ownership. Never silently skips malformed keys/payloads. Raw key
/// bytes are preserved; no RecordId conversion or history export.
///
/// Values pass through the existing storage decoder: its MessagePack projection
/// maps stored `Dec`/`Big` values to `Str` and `Set` values to `List`. This
/// facade cannot recover those source kinds without changing the storage codec.
pub struct LogicalRows<'a> {
    source: RawRows<'a>,
    table: TableManager,
    guard: &'a MaintenanceGuard,
    catalogue: bool,
    done: bool,
}

impl<'a> LogicalRows<'a> {
    pub(super) async fn new(
        table: TableManager,
        version: u64,
        guard: &'a MaintenanceGuard,
        catalogue: bool,
    ) -> DbResult<Self> {
        guard.check_deadline()?;
        // Initialize dictionary before constructing the cursor; it never escapes.
        table.info_store_read_view().interner_read_view().await?;
        let owner = table.clone();
        let source = Box::pin(async_stream::try_stream! {
            if let Some(snapshot) = owner.mvcc_snapshot_stream(1, version) {
                let mut snapshot = std::pin::pin!(snapshot);
                while let Some(batch) = snapshot.next().await {
                    yield batch?;
                }
            } else {
                // Engine-family raw handle: list_stream's relaxed decoder skips
                // invalid keys, which is unacceptable for portable export.
                let mut current = owner.data_store().iter_stream(1);
                while let Some(batch) = current.next().await {
                    yield batch?.into_iter().map(|(key, value)| (Bytes::from(key), value)).collect();
                }
            }
        });
        Ok(Self {
            source,
            table,
            guard,
            catalogue,
            done: false,
        })
    }

    /// `None` is EOF; each nonempty page has exactly one live row. Any error
    /// poisons the cursor, preventing accidental continuation of a partial dump.
    pub async fn next_page(&mut self) -> DbResult<Option<Vec<(Bytes, QueryValue)>>> {
        self.guard.check_deadline()?;
        if self.done {
            return Ok(None);
        }
        self.done = true;
        while let Some(batch) = self.source.next().await {
            let batch = batch?;
            if batch.len() > 1 {
                return Err(DbError::Internal(
                    "logical export source exceeded page size".into(),
                ));
            }
            for (key, bytes) in batch {
                self.guard.check_deadline()?;
                if key.len().saturating_add(bytes.len()) > MAX_ROW_BYTES {
                    return Err(DbError::Validation(
                        "logical export row exceeds frame limit".into(),
                    ));
                }
                // Validate source shape, duplicate interned map keys, and the
                // 64 MiB working budget before either value tree is allocated.
                let decoded_bytes = preflight(&bytes)?;
                let inner =
                    InnerValue::from_bytes(&bytes).map_err(|e| DbError::Codec(e.to_string()))?;
                let interner = self
                    .table
                    .info_store_read_view()
                    .interner_read_view()
                    .await?;
                let mut remaining = MAX_WORKING_BYTES
                    .checked_sub(bytes.len())
                    .and_then(|left| left.checked_sub(decoded_bytes))
                    .ok_or_else(|| {
                        DbError::Validation("logical record exceeds working budget".into())
                    })?;
                named_size(&inner, &interner, &mut remaining)?;
                let value = interner.inner_to_query_value(&inner)?;
                if self.catalogue
                    && self.table.name() == "settings"
                    && !matches!(
                        value.get("key").and_then(QueryValue::as_str),
                        Some("root_meta" | "fn_namespace_meta")
                    )
                {
                    continue;
                }
                if self.catalogue {
                    self.guard.charge_metadata(bytes.len())?;
                }
                self.guard.check_deadline()?;
                self.done = false;
                return Ok(Some(vec![(key, value)]));
            }
        }
        Ok(None)
    }
}

// Bound dictionary-name expansion before allocating a logical-key tree. A
// tiny interned key may resolve to a very large name, repeated in many maps.
fn named_size(
    value: &InnerValue,
    interner: &crate::table::InternerReadView,
    remaining: &mut usize,
) -> DbResult<()> {
    let scalar_bytes = match value {
        InnerValue::Str(value) => value.len(),
        InnerValue::Bin(value) => value.len(),
        _ => 0,
    };
    *remaining = remaining
        .checked_sub(NODE_WORKING_BYTES + scalar_bytes.saturating_mul(2))
        .ok_or_else(|| {
            DbError::Validation("logical record expansion exceeds working budget".into())
        })?;
    match value {
        InnerValue::Map(fields) => {
            for (key, child) in fields {
                let name = interner
                    .get_str(key)
                    .ok_or_else(|| DbError::Codec(format!("unresolved record key {}", key.id())))?;
                *remaining = remaining.checked_sub(name.len()).ok_or_else(|| {
                    DbError::Validation("logical record expansion exceeds limit".into())
                })?;
                named_size(child, interner, remaining)?;
            }
        }
        InnerValue::List(values) => {
            for child in values {
                named_size(child, interner, remaining)?;
            }
        }
        InnerValue::Set(values) => {
            for child in values {
                named_size(child, interner, remaining)?;
            }
        }
        _ => {}
    }
    Ok(())
}

// Allocation-free source-shape validation, not a second dump codec. The
// source frame limit bounds total parsing work; depth and per-container entry
// limits match the format. Interned map keys are checked before serde can
// collapse duplicate keys into the last value.
pub(super) fn preflight(bytes: &[u8]) -> DbResult<usize> {
    let mut position = 0;
    let mut remaining = MAX_WORKING_BYTES
        .checked_sub(bytes.len())
        .ok_or_else(|| DbError::Validation("logical record exceeds working budget".into()))?;
    value(bytes, &mut position, 0, &mut remaining)?;
    if position != bytes.len() {
        return Err(DbError::Codec("trailing record bytes".into()));
    }
    Ok(MAX_WORKING_BYTES - bytes.len() - remaining)
}

fn take<'a>(bytes: &'a [u8], position: &mut usize, length: usize) -> DbResult<&'a [u8]> {
    let end = position
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| DbError::Codec("record length exceeds remaining bytes".into()))?;
    let out = &bytes[*position..end];
    *position = end;
    Ok(out)
}

fn length(bytes: &[u8], position: &mut usize, width: usize) -> DbResult<usize> {
    let mut out = 0usize;
    for byte in take(bytes, position, width)? {
        out = out
            .checked_mul(256)
            .and_then(|n| n.checked_add(usize::from(*byte)))
            .ok_or_else(|| DbError::Codec("record length overflow".into()))?;
    }
    Ok(out)
}

fn interner_key(bytes: &[u8], position: &mut usize) -> DbResult<u64> {
    let marker = take(bytes, position, 1)?[0];
    let width = match marker {
        0xc4 => length(bytes, position, 1)?,
        0xc5 => length(bytes, position, 2)?,
        0xc6 => length(bytes, position, 4)?,
        _ => {
            return Err(DbError::Codec(
                "record map key is not an interned binary key".into(),
            ))
        }
    };
    if !matches!(width, 1 | 2 | 4 | 8) {
        return Err(DbError::Codec("invalid interned map key width".into()));
    }
    let encoded = take(bytes, position, width)?;
    Ok(encoded.iter().enumerate().fold(0u64, |id, (shift, byte)| {
        id | (u64::from(*byte) << (shift * 8))
    }))
}

fn charge(remaining: &mut usize, bytes: usize) -> DbResult<()> {
    *remaining = remaining
        .checked_sub(bytes)
        .ok_or_else(|| DbError::Validation("logical record exceeds working budget".into()))?;
    Ok(())
}

fn value(bytes: &[u8], position: &mut usize, depth: usize, remaining: &mut usize) -> DbResult<()> {
    if depth >= 64 {
        return Err(DbError::Validation(
            "logical record exceeds depth limit".into(),
        ));
    }
    charge(remaining, NODE_WORKING_BYTES)?;
    let marker = take(bytes, position, 1)?[0];
    let (children, map_entries, payload) = match marker {
        0x00..=0x7f | 0xc0 | 0xc2 | 0xc3 | 0xe0..=0xff => (0, None, 0),
        0x80..=0x8f => (0, Some(usize::from(marker & 15)), 0),
        0x90..=0x9f => (usize::from(marker & 15), None, 0),
        0xa0..=0xbf => (0, None, usize::from(marker & 31)),
        0xcc | 0xd0 => (0, None, 1),
        0xcd | 0xd1 => (0, None, 2),
        0xca | 0xce | 0xd2 => (0, None, 4),
        0xcb | 0xcf | 0xd3 => (0, None, 8),
        0xc4 | 0xd9 => (0, None, length(bytes, position, 1)?),
        0xc5 | 0xda => (0, None, length(bytes, position, 2)?),
        0xc6 | 0xdb => (0, None, length(bytes, position, 4)?),
        0xdc => (length(bytes, position, 2)?, None, 0),
        0xdd => (length(bytes, position, 4)?, None, 0),
        0xde | 0xdf => (
            0,
            Some(length(bytes, position, if marker == 0xde { 2 } else { 4 })?),
            0,
        ),
        _ => return Err(DbError::Codec("unsupported record marker".into())),
    };
    let entries = map_entries.unwrap_or(children);
    if entries > MAX_CONTAINER_ENTRIES {
        return Err(DbError::Validation(
            "record container exceeds entry limit".into(),
        ));
    }
    // The limit applies to map members, not the two encoded children (key and
    // value) per member. Keep this count separate from the source-byte bound.
    let encoded_children = map_entries
        .map(|count| {
            count
                .checked_mul(2)
                .ok_or_else(|| DbError::Codec("map length overflow".into()))
        })
        .transpose()?
        .unwrap_or(children);
    if encoded_children > bytes.len() - *position {
        return Err(DbError::Validation(
            "record container exceeds remaining source bytes".into(),
        ));
    }
    take(bytes, position, payload)?;
    charge(remaining, payload)?;
    if let Some(entries) = map_entries {
        let mut seen = TFxSet::<u64>::with_hasher(THasher::default());
        for _ in 0..entries {
            let key = interner_key(bytes, position)?;
            charge(remaining, MAP_KEY_WORKING_BYTES)?;
            if !seen.insert(key) {
                return Err(DbError::Codec("duplicate interned record map key".into()));
            }
            value(bytes, position, depth + 1, remaining)?;
        }
    } else {
        for _ in 0..children {
            value(bytes, position, depth + 1, remaining)?;
        }
    }
    Ok(())
}
