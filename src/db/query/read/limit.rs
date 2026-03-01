//! Pagination — LIMIT/OFFSET and page-based pagination.

use serde::{Deserialize, Serialize};

/// Pagination mode for queries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode")]
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
    /// No pagination
    None,
}

impl Pagination {
    /// Resolve to (skip, take) pair
    pub fn resolve(&self) -> (u64, Option<u64>) {
        match self {
            Pagination::LimitOffset { limit, offset } => (*offset, *limit),
            Pagination::Page { page, page_size } => {
                let skip = page.saturating_sub(1) * page_size;
                (skip, Some(*page_size))
            }
            Pagination::None => (0, Option::None),
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

    /// Check if this is the default (no pagination)
    pub fn is_none(&self) -> bool {
        matches!(self, Pagination::None)
    }
}

impl Default for Pagination {
    fn default() -> Self {
        Self::None
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
                let total_pages = (total + page_size - 1) / page_size;
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
