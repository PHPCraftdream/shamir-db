//! `event_from_wal_entry` - the single journal-event derivation.

use shamir_wal::{WalEntryV2, WalOpV2};

use crate::changefeed::{ChangeOp, ChangelogEvent, RecordChange};

/// Derive the journal event of a committed transaction from its WAL
/// entry. This is THE derivation (S5c-X I1): the live commit path calls
/// it on the entry it just built, and cold recovery (PR4) calls it on
/// entries recovered from disk. There is no second projection.
///
/// # Exact mapping (entry -> event)
///
/// - `entry.event == None` (format versions 1-2, or a pre-S5c writer)
///   returns `None` - "unknown": no event can be derived, recovery treats
///   the commit conservatively (GAP fallback). This is why the header is
///   necessary, not optional.
/// - `repo` = `header.repo`, `commit_version` = `entry.commit_version`,
///   `tx_id` = `header.tx_id` (already the PUBLIC form: 0 for an implicit
///   batch write, the real id otherwise), `actor` = `header.actor`,
///   `timestamp_ns` = `header.timestamp_ns` (sampled once at entry
///   construction, strictly before the WAL append - it can only be
///   EARLIER than the old post-acceptance sampling moment, never later,
///   and it is never used for ordering; `commit_version` orders events).
/// - `changes` are produced by walking `entry.ops` IN ORDER. Every
///   `WalOpV2::Put`/`Delete` maps 1:1 to a `RecordChange` (`Put` carries
///   the FULL new record bytes as `value`, `Delete` carries `None`; the
///   key is the `RecordId`'s 16 bytes - byte-identical to the staged
///   storage key, since `WalOpV2::Put`/`Delete` exist only for keys that
///   parse as a 16-byte `RecordId`). Every non-`RecordId` staged write is
///   carried verbatim in `header.extra_changes` with the 0-based position
///   it had among ALL of the commit's projected changes; derivation
///   splices those back at exactly that position. The resulting change
///   order is therefore byte-for-byte the order the live projection
///   (`project_event`) emits: tables in `write_set` iteration order, ops
///   in per-table staging order.
/// - Table names resolve from `header.table_names`; a token absent there
///   falls back to `token:{t}` - the same fallback `project_event` uses.
///
/// # Eventless commits
///
/// Returns `None` when the commit carries NO record-level change: an
/// empty footprint, or an index-only / counter-only / interner-only
/// footprint (no `Put`/`Delete` ops and no `extra_changes`). Such commits
/// still write WAL entries (data + index ops) but never journal events -
/// identical to the live contract.
pub fn event_from_wal_entry(entry: &WalEntryV2) -> Option<ChangelogEvent> {
    let header = entry.event.as_ref()?;

    // Non-RecordId changes first, keyed by their projected position.
    let mut extras: Vec<(u32, RecordChange)> = header
        .extra_changes
        .iter()
        .map(|c| {
            (
                c.change_index,
                RecordChange {
                    table: table_name(header, c.table_id_interned),
                    key: c.key.clone(),
                    op: if c.value.is_some() {
                        ChangeOp::Put
                    } else {
                        ChangeOp::Delete
                    },
                    value: c.value.clone(),
                },
            )
        })
        .collect();
    // Generated in increasing order; sort defensively so the splice below
    // can rely on it.
    extras.sort_by_key(|(i, _)| *i);

    // RecordId-keyed changes, in op order.
    let mut rid_changes: Vec<RecordChange> = Vec::new();
    for op in &entry.ops {
        match op {
            WalOpV2::Put {
                table_id_interned,
                rid,
                body,
            } => rid_changes.push(RecordChange {
                table: table_name(header, *table_id_interned),
                key: rid.to_bytes(),
                op: ChangeOp::Put,
                value: Some(body.clone()),
            }),
            WalOpV2::Delete {
                table_id_interned,
                rid,
            } => rid_changes.push(RecordChange {
                table: table_name(header, *table_id_interned),
                key: rid.to_bytes(),
                op: ChangeOp::Delete,
                value: None,
            }),
            _ => {}
        }
    }

    if rid_changes.is_empty() && extras.is_empty() {
        return None;
    }

    // Splice: the RecordId changes occupy exactly the positions the
    // extras do not, in increasing order (both walks iterate the same
    // `write_set` -> `iter_ops` order the live projection used).
    let total = rid_changes.len() + extras.len();
    let mut changes: Vec<RecordChange> = Vec::with_capacity(total);
    let mut extras_iter = extras.into_iter().peekable();
    let mut rid_iter = rid_changes.into_iter();
    for slot in 0..total {
        let slot = slot as u32;
        match extras_iter.peek() {
            Some((i, _)) if *i == slot => {
                changes.push(extras_iter.next().expect("peeked").1);
            }
            _ => match rid_iter.next() {
                Some(c) => changes.push(c),
                // Defensive: a header with an out-of-range duplicate
                // change_index (not producible by `event_header_from_tx`)
                // must not panic a recovery caller; flush the remainder
                // in stored order instead.
                None => {
                    changes.extend(extras_iter.map(|(_, c)| c));
                    break;
                }
            },
        }
    }

    Some(ChangelogEvent {
        repo: header.repo.clone(),
        commit_version: entry.commit_version,
        tx_id: header.tx_id,
        actor: header.actor.clone(),
        timestamp_ns: header.timestamp_ns,
        changes,
    })
}

/// Resolve a table token's human-readable name, falling back to
/// `token:{t}` exactly like the live projection.
fn table_name(header: &shamir_wal::WalEventHeader, token: u64) -> String {
    header
        .table_names
        .iter()
        .find(|(t, _)| *t == token)
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| format!("token:{token}"))
}
