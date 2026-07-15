//! [`ForEachOp`] — a data-dependent loop: execute a nested [`BatchRequest`]
//! once per element of `over`, binding the current element to `bind_row`.

use serde::{Deserialize, Serialize};

use crate::filter::FilterValue;

use super::batch_request::BatchRequest;

/// A `for_each` loop — runs `batch` once per element of `over`, with the
/// current element bound (as a `$param`) under the name `bind_row`.
///
/// Structurally a sibling of [`SubBatchOp`](super::SubBatchOp): `batch` is
/// reused verbatim, and `bind_row` plays the same "current scope injected
/// parameter" role as `SubBatchOp::bind`'s values — the difference is that
/// `ForEachOp` runs the body K times (once per element of `over`) instead
/// of once, per
/// `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md` Decision 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForEachOp {
    /// The value-producing expression that yields the list of elements to
    /// iterate over (e.g. `@alias[].field` or a literal array).
    pub over: FilterValue,

    /// The name under which the current iteration's element is exposed to
    /// the body as a `$param` (resolved the same way `SubBatchOp::bind`'s
    /// values are resolved today).
    pub bind_row: String,

    /// The loop body, planned once and executed K times (ADR Decision 1).
    ///
    /// Wire-renamed to `for_each` (not `batch`) so the dispatcher in
    /// `batch_op.rs` can distinguish a `ForEachOp` from a `SubBatchOp` by
    /// wire key alone — both structs would otherwise emit a `"batch"` key,
    /// making them indistinguishable on the wire.
    #[serde(rename = "for_each")]
    pub batch: BatchRequest,
}
