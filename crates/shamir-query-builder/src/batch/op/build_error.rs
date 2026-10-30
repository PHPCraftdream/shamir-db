/// Client-side validation error from [`super::Batch::try_build`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BuildError {
    /// A `$query` ref points to an alias not present in the batch.
    #[error("unknown alias '{alias}' referenced by '{referenced_by}'")]
    UnknownAlias {
        /// The alias that was referenced.
        alias: String,
        /// The alias of the entry that contains the bad reference.
        referenced_by: String,
    },
    /// A `$query` ref inside an entry points back to itself.
    #[error("alias '{alias}' references itself")]
    SelfReference {
        /// The alias that references itself.
        alias: String,
    },
    /// An `after` entry carried a value-path tail (e.g. `"mk[0].id"`,
    /// `"mk.id"`) that `after` silently ignores.
    ///
    /// `after` is alias-only ordering — it never resolves a value path the
    /// way `$query` does. A path tail here is almost always a developer
    /// mistake, so the builder rejects it up front (mirrors
    /// `shamir_query_types::batch::BatchError::AfterPathIgnored`).
    #[error("'after' entry '{raw}' on '{alias}' carries a value-path tail, but 'after' is alias-only ordering and never resolves a path; use a bare alias, or a '$query' reference if you need the value")]
    AfterPathIgnored {
        /// The alias of the entry whose `after` list carries the bad ref.
        alias: String,
        /// The raw `after` string that carried the path tail.
        raw: String,
    },
    /// A registration call reused an alias that is already registered in the
    /// batch. The FIRST registered operation is kept intact and the later
    /// registration is discarded; the duplicate is recorded here as a deferred
    /// error surfaced by [`Batch::try_build`], so re-registering an alias can
    /// never silently replace (and thus drop) an earlier op.
    #[error("alias '{alias}' is already registered in this batch; the first registered operation is kept and the duplicate was discarded")]
    DuplicateAlias {
        /// The alias that was registered twice.
        alias: String,
    },
    /// A handle-consuming API (`after`/`when`) received a handle whose alias
    /// is not registered in this batch. The call is discarded and recorded as
    /// a deferred error surfaced by [`Batch::try_build`] — unknown handles are
    /// errors, not silent no-ops.
    #[error("'{operation}' received a handle for alias '{alias}', which is not registered in this batch")]
    UnknownOrderingTarget {
        /// The alias the handle carried (not registered in the batch).
        alias: String,
        /// The attempted operation: `"after"` or `"when"`.
        operation: &'static str,
    },
    /// A msgpack round-trip used by validation, or request encoding, failed.
    /// An entry can hold a value msgpack cannot represent (e.g. a `QueryValue`
    /// carrying a non-finite float, or a map with non-string keys) — surface
    /// it as a typed error instead of panicking, since `try_build` exists
    /// specifically so a malformed batch produces a `Result::Err`, not a
    /// panic, at the client's own validation call site.
    #[error("entry '{alias}' could not be validated: {reason}")]
    SerializationFailed {
        /// The alias of the entry that failed to serialize/deserialize.
        alias: String,
        /// The underlying codec error, as a string (codec errors aren't
        /// `Clone`, and `BuildError` derives `PartialEq`/`Clone`).
        reason: String,
    },
}
