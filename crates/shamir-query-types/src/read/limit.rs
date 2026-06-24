//! Pagination — LIMIT/OFFSET, page-based, and keyset (seek) pagination.

use serde::{Deserialize, Serialize};
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
                },
                Pagination::After {
                    key: k2,
                    limit: lim2,
                },
            ) => lim1 == lim2 && key_bytes(k1) == key_bytes(k2),
            (Pagination::None, Pagination::None) => true,
            _ => false,
        }
    }
}

/// Encode a seek-tuple slice to canonical MessagePack bytes for equality.
fn key_bytes(key: &[QueryValue]) -> Vec<u8> {
    rmp_serde::to_vec_named(key).expect("serializing Vec<QueryValue> is infallible")
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
            Pagination::After { key, limit } => Some((key.as_slice(), *limit)),
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
        Pagination::After { key, limit }
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
