//! Pagination — LIMIT/OFFSET, page-based, and keyset (seek) pagination.

use serde::{Deserialize, Serialize};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

/// Pagination mode for queries.
///
/// Note: this enum is **not** `Copy` because the [`Pagination::After`] variant
/// owns a `Vec<QueryValue>` seek key. Call sites already pass `Pagination` by
/// reference or by move, so dropping `Copy` has no ripple beyond the derive
/// list itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode")]
#[derive(Default)]
pub enum Pagination {
    /// Classic limit + offset
    LimitOffset {
        /// Maximum records to return
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u64>,
        /// Records to skip
        #[serde(default)]
        offset: u64,
    },
    /// Page-based: page number (1-based) + page size
    Page {
        /// Page number (1-based)
        page: u64,
        /// Number of records per page
        page_size: u64,
    },
    /// Keyset / seek pagination: return up to `limit` rows ordered strictly
    /// after the tuple `key`. `key` is the ordered tuple of values matching
    /// the query's ORDER BY columns.
    ///
    /// Seek semantics do not map onto the `(skip, take)` model exposed by
    /// [`Pagination::resolve`]; use [`Pagination::keyset`] to inspect the seek
    /// tuple. The planner consumes `keyset()` in a downstream task.
    ///
    /// Wire tag is `"After"` (PascalCase) — consistent with the sibling
    /// `LimitOffset` / `Page` / `None` variants, which use the default
    /// serde-variant-name tag (no rename).
    After {
        /// Ordered seek tuple (one value per ORDER BY column).
        key: Vec<QueryValue>,
        /// Maximum records to return after the seek tuple.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u64>,
        /// Optional record-id tie-breaker (task #537). When the ORDER BY
        /// value(s) in `key` are shared by several rows across a page
        /// boundary, `key` alone cannot distinguish "the row the client
        /// already has" from "a different row tied on the same value" — so
        /// the value-only skip drops every tied row permanently. A client
        /// that echoes back the `_id` of the last row it received sets
        /// `after_id` to resume STRICTLY past that specific row instead of
        /// past the bare value.
        ///
        /// **Backward-compatible / additive.** `None` (the default, and what
        /// every old client / old query-builder sends) reproduces today's
        /// exact skip-all-ties behavior — a pre-existing known limitation for
        /// those callers, never a new regression. Only clients that opt in by
        /// echoing the id get correct tie-breaking. `skip_serializing_if`
        /// keeps the wire shape byte-identical when absent.
        ///
        /// **Wire form is the base58 STRING** (same token the client received
        /// as the row's `_id` in read results — `_id` is emitted as a base58
        /// string by `InsertedRecord`). This lets a client echo the exact
        /// `_id` value verbatim; `RecordId`'s own `Deserialize` expects 16
        /// raw bytes, so we round-trip through `Display`/`FromStr` instead
        /// (see [`opt_record_id_base58`]).
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "opt_record_id_base58"
        )]
        after_id: Option<RecordId>,
    },
    /// No pagination
    #[default]
    None,
}

/// Manual equality because `QueryValue` does not implement `PartialEq`.
///
/// For the `After` variant the seek tuple is compared by canonical
/// MessagePack encoding (`rmp_serde::to_vec_named`); the offset-based variants
/// compare their `u64` fields directly.
impl PartialEq for Pagination {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Pagination::LimitOffset {
                    limit: l1,
                    offset: o1,
                },
                Pagination::LimitOffset {
                    limit: l2,
                    offset: o2,
                },
            ) => l1 == l2 && o1 == o2,
            (
                Pagination::Page {
                    page: p1,
                    page_size: s1,
                },
                Pagination::Page {
                    page: p2,
                    page_size: s2,
                },
            ) => p1 == p2 && s1 == s2,
            (
                Pagination::After {
                    key: k1,
                    limit: lim1,
                    after_id: aid1,
                },
                Pagination::After {
                    key: k2,
                    limit: lim2,
                    after_id: aid2,
                },
            ) => lim1 == lim2 && aid1 == aid2 && key_bytes(k1) == key_bytes(k2),
            (Pagination::None, Pagination::None) => true,
            _ => false,
        }
    }
}

/// Encode a seek-tuple slice to canonical MessagePack bytes for equality.
fn key_bytes(key: &[QueryValue]) -> Vec<u8> {
    rmp_serde::to_vec_named(key).expect("serializing Vec<QueryValue> is infallible")
}

/// Serde `with`-module: represent `Option<RecordId>` on the wire as an
/// optional base58 **string** (task #537).
///
/// The read-result `_id` a client echoes back is a base58 string (emitted by
/// `InsertedRecord`); `RecordId`'s own `Serialize`/`Deserialize` uses 16 raw
/// bytes, which would NOT accept that string. This module bridges the two so
/// `after_id` round-trips the same token the client received.
mod opt_record_id_base58 {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use shamir_types::types::record_id::RecordId;

    pub(super) fn serialize<S: Serializer>(
        v: &Option<RecordId>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        // `skip_serializing_if = Option::is_none` guarantees `Some` here, but
        // stay total: emit the base58 string for `Some`, unit for `None`.
        match v {
            Some(id) => id.to_string().serialize(s),
            None => s.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<RecordId>, D::Error> {
        let opt = Option::<String>::deserialize(d)?;
        match opt {
            Some(s) => s
                .parse::<RecordId>()
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

impl Pagination {
    /// Resolve to (skip, take) pair.
    ///
    /// For [`Pagination::After`] this returns `(0, limit)` — seek semantics
    /// are **not** expressible as (skip, take) and are consumed by the planner
    /// via [`Pagination::keyset`]. The `(0, limit)` value keeps `resolve`
    /// total for existing offset-based call sites (limit still caps the page).
    pub fn resolve(&self) -> (u64, Option<u64>) {
        match self {
            Pagination::LimitOffset { limit, offset } => (*offset, *limit),
            Pagination::Page { page, page_size } => {
                let skip = page.saturating_sub(1) * page_size;
                (skip, Some(*page_size))
            }
            Pagination::After { limit, .. } => (0, *limit),
            Pagination::None => (0, Option::None),
        }
    }

    /// Seek-tuple accessor for keyset pagination.
    ///
    /// Returns `Some((key_slice, limit))` for [`Pagination::After`], and
    /// `None` for every other variant. The planner uses this to build a
    /// strict-prefix range scan over the ORDER BY key space.
    pub fn keyset(&self) -> Option<(&[QueryValue], Option<u64>)> {
        match self {
            Pagination::After { key, limit, .. } => Some((key.as_slice(), *limit)),
            _ => None,
        }
    }

    /// Record-id tie-breaker accessor for keyset pagination (task #537).
    ///
    /// Returns `Some(&record_id)` only when this is a [`Pagination::After`]
    /// carrying an `after_id`, and `None` otherwise (including an `After` that
    /// omitted the tie-breaker — an old client, reproducing today's
    /// skip-all-ties behavior). The keyset-seek executor uses this to bound
    /// the physical-key range scan by `(seek_value, after_id)` instead of the
    /// approximate value-only filter.
    pub fn after_id(&self) -> Option<&RecordId> {
        match self {
            Pagination::After { after_id, .. } => after_id.as_ref(),
            _ => None,
        }
    }

    /// No pagination
    pub fn no_limit() -> Self {
        Pagination::None
    }

    /// Page-based pagination
    pub fn page(page: u64, page_size: u64) -> Self {
        Pagination::Page { page, page_size }
    }

    /// Keyset / seek pagination: return up to `limit` rows ordered after the
    /// tuple `key`.
    pub fn after(key: Vec<QueryValue>, limit: Option<u64>) -> Self {
        Pagination::After {
            key,
            limit,
            after_id: None,
        }
    }

    /// Keyset / seek pagination WITH a record-id tie-breaker (task #537):
    /// return up to `limit` rows ordered STRICTLY after the row identified by
    /// `(key, after_id)`.
    ///
    /// `after_id` is the `_id` of the last row the client received on the
    /// previous page (surfaced in read results — see `QueryRecord`). Passing
    /// it lets the server resume past that exact row rather than past the bare
    /// ORDER BY value, so rows tied on the same value across a page boundary
    /// are no longer silently dropped. `after(key, limit)` (no tie-breaker)
    /// stays the backward-compatible default.
    pub fn after_with_id(
        key: Vec<QueryValue>,
        limit: Option<u64>,
        after_id: Option<RecordId>,
    ) -> Self {
        Pagination::After {
            key,
            limit,
            after_id,
        }
    }

    /// Check if this is the default (no pagination)
    pub fn is_none(&self) -> bool {
        matches!(self, Pagination::None)
    }
}

/// Pagination metadata for query results
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaginationInfo {
    /// Total number of matching records (only if count_total was requested)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_count: Option<u64>,
    /// Total number of pages
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_pages: Option<u64>,
    /// Current page number (if page-based)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_page: Option<u64>,
    /// Page size (if page-based or limit is set)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u64>,
    /// Whether there are more results after this page
    pub has_next: bool,
    /// Whether there are results before this page
    pub has_prev: bool,
}

impl PaginationInfo {
    /// Compute pagination info from pagination mode and total count.
    /// `total_count` is `Some` only if the caller requested it.
    pub fn compute(pagination: &Pagination, total_count: Option<u64>) -> Self {
        let (skip, take) = pagination.resolve();
        let has_prev = skip > 0;

        match (take, total_count) {
            (Some(page_size), Some(total)) if page_size > 0 => {
                let total_pages = total.div_ceil(page_size);
                let has_next = skip + page_size < total;
                let current_page = match pagination {
                    Pagination::Page { page, .. } => Some(*page),
                    _ => Option::None,
                };
                PaginationInfo {
                    total_count: Some(total),
                    total_pages: Some(total_pages),
                    current_page,
                    page_size: Some(page_size),
                    has_next,
                    has_prev,
                }
            }
            (Some(page_size), Option::None) if page_size > 0 => {
                // No total count — we can't compute total_pages, but can still
                // provide page_size and current_page
                let current_page = match pagination {
                    Pagination::Page { page, .. } => Some(*page),
                    _ => Option::None,
                };
                PaginationInfo {
                    total_count: Option::None,
                    total_pages: Option::None,
                    current_page,
                    page_size: Some(page_size),
                    has_next: false, // unknown without total — caller should set via has_next_hint
                    has_prev,
                }
            }
            _ => PaginationInfo {
                total_count,
                total_pages: Option::None,
                current_page: Option::None,
                page_size: Option::None,
                has_next: false,
                has_prev,
            },
        }
    }

    /// Set has_next hint (when determined by fetching N+1 rows)
    pub fn with_has_next(mut self, has_next: bool) -> Self {
        self.has_next = has_next;
        self
    }
}
