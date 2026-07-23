//! [`BatchError`] — errors that can occur during batch processing.

/// Errors during batch processing.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchError {
    /// Too many queries in batch.
    ///
    /// Check `BatchLimits::max_queries`.
    TooManyQueries { count: usize, max: usize },

    /// Circular dependency detected.
    ///
    /// The `cycle` field contains the cycle path, e.g., `["a", "b", "c", "a"]`.
    CircularDependency { cycle: Vec<String> },

    /// Dependency depth exceeded.
    ///
    /// Check `BatchLimits::max_dependency_depth`.
    TooDeep { depth: usize, max: usize },

    /// Unknown alias referenced.
    ///
    /// A query referenced an alias that doesn't exist in the batch.
    UnknownAlias {
        alias: String,
        referenced_by: String,
    },

    /// Execution timeout.
    ///
    /// Total execution time exceeded `BatchLimits::max_execution_time_secs`.
    Timeout { elapsed_secs: u64 },

    /// Query execution error.
    ///
    /// `code` carries a machine-readable error category when available
    /// (e.g. `"exists"`, `"not_found"`, `"access_denied"`,
    /// `"still_referenced"`).  Unclassified errors leave it `none`.
    QueryError {
        alias: String,
        message: String,
        #[doc(hidden)]
        code: Option<String>,
    },

    /// Lock timeout (deadlock prevention).
    ///
    /// Could not acquire locks within the timeout period.
    LockTimeout { aliases: Vec<String> },

    /// Transactional batch targets more than one repository.
    ///
    /// 2PC across repos is intentionally out of scope. Clients must
    /// split such batches into separate single-repo transactions.
    CrossRepoNotSupported { repos: Vec<String> },

    /// Static sub-batch nesting depth exceeded.
    ///
    /// The op tree contains `BatchOp::Batch` nodes nested deeper than
    /// `BatchLimits::max_nesting_depth`.
    NestingTooDeep { depth: usize, max: usize },

    /// An `after` entry carried a value-path tail (e.g. `"mk[0].id"`,
    /// `"mk.id"`) that `after` silently ignores.
    ///
    /// `after` is alias-only ordering — it never resolves a value path the
    /// way `$query` does. A path tail here is almost always a developer
    /// mistake ("I thought `after` pointed at a specific field"), so we
    /// reject it at planning time instead of silently stripping to the base
    /// alias.
    AfterPathIgnored { alias: String, raw: String },

    /// A `for_each` loop's `over` resolved to more elements than
    /// `BatchLimits::max_iterations` allows.
    ///
    /// Checked at runtime, immediately BEFORE iteration 0 — never a partial
    /// run followed by a mid-loop abort (ADR
    /// `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md` Decision 3).
    TooManyIterations {
        alias: String,
        actual: usize,
        max: usize,
    },

    /// #651: `entry.when` contains an old record-field-based comparison
    /// variant (`Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`/`FieldEq`).
    ///
    /// `when` has no per-row record to resolve a `FieldPath` against — a
    /// field-based comparison there ALWAYS folded (silently, before this
    /// error existed) to a fixed result, since `compile_filter` compiles it
    /// against an empty scratch interner. This turns that silent-wrong-
    /// answer bug into a caught, explicit plan-time error.
    InvalidWhenFilter { alias: String, message: String },

    /// #663: a `$cond` marker embedded inside a write value
    /// (`InsertOp.values`/`UpdateOp.set`/`SetOp.{key,value}`) has a
    /// `condition` that contains an old record-field-based comparison
    /// variant.
    ///
    /// Write-value `$cond` resolution (`resolve_write_value` in
    /// `shamir-engine`'s `param_subst.rs`) evaluates `condition` against the
    /// SAME kind of record-less dummy `when`'s `resolve_skip` does — a
    /// field-based comparison there ALWAYS folds (silently) to a fixed
    /// result instead of erroring, exactly the #651 class of bug just one
    /// level deeper. This turns it into a caught, explicit plan-time error.
    InvalidCondCondition { alias: String, message: String },

    /// #666: the batch's total execution time exceeded
    /// `BatchLimits.max_execution_time_secs`.
    ///
    /// Raised by a COOPERATIVE deadline checkpoint (shamir-engine's
    /// `ExecutionDeadline::check`, consulted before each stage-alias
    /// dispatch, each `ForEach` iteration, each nested-body entry, and
    /// immediately before commit) — never by external future cancellation.
    /// It flows through the executor's ordinary error path, so for a
    /// transactional batch it reaches `execute_transactional_impl`'s
    /// existing `Err` arm: pessimistic locks are released, `commit_tx` is
    /// never called, and the `TxContext` is dropped without commit (RAII
    /// rollback) — the SAME cleanup any other op failure gets. In
    /// particular this error is only ever produced BEFORE the commit
    /// decision: a batch that reports `ExecutionTimedOut` has durably
    /// committed nothing.
    ExecutionTimedOut { budget_secs: u64 },

    /// FG-5a: `FetchNext`/`CancelCursor` against a cursor id the server has
    /// never issued (or has already fully consumed/dropped for a reason
    /// other than idle-timeout eviction — see `CursorExpired` for that
    /// case).
    ///
    /// Wire error code (see `crate::wire::db_message::DbResponse::Error`):
    /// `cursor_not_found`.
    CursorNotFound {
        /// The unrecognised cursor id.
        cursor_id: crate::wire::CursorId,
    },

    /// FG-5a: `FetchNext` against a cursor the server evicted after it sat
    /// idle past its idle-timeout. Distinguishable from `CursorNotFound` so
    /// a client can tell "you waited too long" apart from "that id was
    /// never valid" — eviction itself is implemented in FG-5b; this variant
    /// only reserves the wire-distinguishable error code.
    ///
    /// Wire error code: `cursor_expired`.
    CursorExpired {
        /// The evicted cursor id.
        cursor_id: crate::wire::CursorId,
    },

    /// FG-5a: `CreateCursor` rejected because the caller's session already
    /// has `limit` cursors open. Cap enforcement itself lands in FG-5b;
    /// this variant only reserves the wire-distinguishable error code.
    ///
    /// Wire error code: `cursor_limit_exceeded`.
    CursorLimitExceeded {
        /// The per-session cap that was hit.
        limit: u32,
    },

    /// FG-5b: `CreateCursor` rejected because `ReadQuery.temporal` was
    /// `AsOf { .. }` or `History { .. }` instead of the default `Latest`.
    ///
    /// This is a DELIBERATE, DOCUMENTED scope cut (see
    /// `docs/dev-artifacts/prompts/post-alpha/03-fg5b-engine-session-cursor.md`
    /// §2): a cursor pins its snapshot via `RepoTxGate::open_snapshot()`,
    /// which only pins "whatever is currently committed" — there is no API
    /// to pin an arbitrary already-past version on demand, and a historical
    /// version may already be past the MVCC GC floor by the time a cursor
    /// asks for it. `Temporal::AsOf`/`Temporal::History` on a plain read go
    /// through separate one-shot, non-resumable code paths not designed for
    /// incremental keyset pagination. Rather than silently downgrading the
    /// caller's request to `Latest` (a wrong-results bug), `CreateCursor`
    /// rejects it outright with this distinct, named error. A future task
    /// can revisit full historical-cursor support if ever needed.
    ///
    /// Wire error code: `cursor_temporal_not_supported`.
    CursorTemporalNotSupported,

    /// CR-B5 (#771): `CreateCursor` rejected because `ReadQuery.with_version`
    /// was `true`.
    ///
    /// `with_version = true` requests a per-record version stamp on each
    /// returned row, for later optimistic-CAS use (the FG-2 contour, see
    /// `docs/guide-docs/client-server-protocol-spec/OPTIMISTIC_CONCURRENCY.md`).
    /// A cursor's every internal read (both `create_cursor`'s first page and
    /// every `fetch_next`) goes through `Temporal::AsOf { at:
    /// At::Version(pinned_version) }`, and that read path
    /// (`TableManager::read_as_of` in `shamir-engine`'s `read_temporal.rs`,
    /// its final `QueryResult` construction) hard-codes `versions: None`.
    /// Honoring `with_version` through a cursor would therefore either
    /// silently produce no versions (the bug this variant closes) or require
    /// threading real historical per-record versions through the whole
    /// `AsOf` pipeline — out of scope here. Rather than silently downgrading
    /// the caller's request (a correctness-relevant feature quietly stops
    /// working), `CreateCursor` rejects the combination outright with this
    /// distinct, named error, mirroring `CursorTemporalNotSupported`'s
    /// scope-cut precedent immediately above.
    ///
    /// A future task could thread REAL historical versions through the
    /// `AsOf` pipeline so cursors can honor `with_version` too — tracked as
    /// a possible follow-up, not attempted here.
    ///
    /// Wire error code: `cursor_with_version_not_supported`.
    CursorWithVersionNotSupported,

    /// CR-A3: `CreateCursor`/`FetchNext` rejected because `page_size` was
    /// outside the valid `1..=max` range.
    ///
    /// `page_size == 0` previously caused an infinite loop: `has_more` is
    /// computed as `page.records.len() as u64 >= page_size as u64`, and with
    /// `page_size == 0` that is `0 >= 0 → true` forever — the bookmark never
    /// advances, so the page is always empty and `has_more` never becomes
    /// `false`. `page_size` above `max` is rejected (not silently clamped) —
    /// a client that thinks it got `page_size` rows per page but silently
    /// got fewer would misinterpret `has_more` semantics.
    ///
    /// Wire error code: `invalid_page_size`.
    InvalidPageSize {
        /// The rejected `page_size` value.
        page_size: u32,
        /// The configured `max_cursor_page_size` cap (the valid range is
        /// `1..=max`).
        max: u32,
    },

    /// CR-A5: a `CreateCursor`/`FetchNext` page's serialized size exceeded
    /// `security.query_limits.max_result_size_bytes`.
    ///
    /// The page is rejected outright, never truncated — truncating would
    /// silently corrupt the `has_more`/bookmark contract (a truncated page
    /// would advance the bookmark past records the caller never actually
    /// received), the same reasoning CR-A3 used for `page_size`. Measured
    /// once (the same serialization the byte-budget acquire would otherwise
    /// perform) and rejected BEFORE any budget bytes are reserved for it —
    /// there is nothing to write, so nothing should be charged.
    ///
    /// Wire error code: `cursor_page_too_large`.
    CursorPageTooLarge {
        /// The rejected page's serialized size, in bytes.
        size: usize,
        /// The configured `max_result_size_bytes` cap.
        max: usize,
    },
}

impl std::fmt::Display for BatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BatchError::TooManyQueries { count, max } => {
                write!(f, "Too many queries: {} (max: {})", count, max)
            }
            BatchError::CircularDependency { cycle } => {
                write!(f, "Circular dependency: {}", cycle.join(" -> "))
            }
            BatchError::TooDeep { depth, max } => {
                write!(f, "Dependency depth too deep: {} (max: {})", depth, max)
            }
            BatchError::UnknownAlias {
                alias,
                referenced_by,
            } => {
                write!(
                    f,
                    "Unknown alias '{}' referenced by '{}'",
                    alias, referenced_by
                )
            }
            BatchError::Timeout { elapsed_secs } => {
                write!(f, "Execution timeout after {}s", elapsed_secs)
            }
            BatchError::QueryError {
                alias,
                message,
                code,
            } => {
                if let Some(c) = code {
                    write!(f, "Query '{}' failed [{}]: {}", alias, c, message)
                } else {
                    write!(f, "Query '{}' failed: {}", alias, message)
                }
            }
            BatchError::LockTimeout { aliases } => {
                write!(f, "Lock timeout for queries: {}", aliases.join(", "))
            }
            BatchError::CrossRepoNotSupported { repos } => write!(
                f,
                "transactional batch targets multiple repositories ({}); single-repo only",
                repos.join(", ")
            ),
            BatchError::NestingTooDeep { depth, max } => {
                write!(f, "Sub-batch nesting too deep: {} (max: {})", depth, max)
            }
            BatchError::AfterPathIgnored { alias, raw } => {
                write!(
                    f,
                    "'after' entry '{}' on '{}' carries a value-path tail, but 'after' is \
                     alias-only ordering and never resolves a path; use a bare alias, or a \
                     '$query' reference if you need the value",
                    raw, alias
                )
            }
            BatchError::TooManyIterations { alias, actual, max } => {
                write!(
                    f,
                    "'for_each' loop '{}' resolved {} iterations, exceeding max_iterations ({})",
                    alias, actual, max
                )
            }
            BatchError::InvalidWhenFilter { alias, message } => {
                write!(f, "invalid 'when' filter on '{}': {}", alias, message)
            }
            BatchError::InvalidCondCondition { alias, message } => {
                write!(
                    f,
                    "invalid '$cond' condition in write value on '{}': {}",
                    alias, message
                )
            }
            BatchError::ExecutionTimedOut { budget_secs } => {
                write!(
                    f,
                    "batch execution exceeded its {}s time budget",
                    budget_secs
                )
            }
            BatchError::CursorNotFound { cursor_id } => {
                write!(f, "cursor {} not found", cursor_id)
            }
            BatchError::CursorExpired { cursor_id } => {
                write!(f, "cursor {} expired (idle-timeout eviction)", cursor_id)
            }
            BatchError::CursorLimitExceeded { limit } => {
                write!(f, "cursor limit exceeded (max: {})", limit)
            }
            BatchError::CursorTemporalNotSupported => {
                write!(
                    f,
                    "CreateCursor only supports Temporal::Latest queries (a cursor pins a \
                     live MVCC snapshot; AsOf/History cursors are out of scope for now — see \
                     FG-5b)"
                )
            }
            BatchError::CursorWithVersionNotSupported => {
                write!(
                    f,
                    "CreateCursor does not support with_version = true (a cursor's internal \
                     reads use the AsOf temporal path, which does not attach per-record \
                     versions — see FG-5b / CR-B5)"
                )
            }
            BatchError::InvalidPageSize { page_size, max } => {
                write!(
                    f,
                    "page_size must be between 1 and {} (got {})",
                    max, page_size
                )
            }
            BatchError::CursorPageTooLarge { size, max } => {
                write!(
                    f,
                    "cursor page too large: {} bytes exceeds max_result_size_bytes ({})",
                    size, max
                )
            }
        }
    }
}

impl std::error::Error for BatchError {}

impl BatchError {
    /// Structured DDL/admin error with a machine-readable `code`.
    pub fn query_coded(
        alias: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        BatchError::QueryError {
            alias: alias.into(),
            message: message.into(),
            code: Some(code.into()),
        }
    }

    /// Return the machine-readable code, if set.
    pub fn code(&self) -> Option<&str> {
        match self {
            BatchError::QueryError { code, .. } => code.as_deref(),
            _ => None,
        }
    }
}
