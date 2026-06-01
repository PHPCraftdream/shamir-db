//! Execution-context handles for the function engine.
//!
//! Slice 1 keeps these minimal so the `(ctx, batch, params)` contract has
//! its final SHAPE while the substance is wired in later slices:
//!
//! * [`FnCtx`] grows to carry the DBMS handle bound to the current
//!   `TxContext` (so a function's reads/writes join the same MVCC snapshot,
//!   read-/write-set, SSI and predicate locks), plus the principal and the
//!   snapshot clock.
//! * [`FnBatch`] grows to carry the `@alias` namespace (read prior nodes,
//!   `put` scratch values) and the tx staging buffer (`append` new ops).
//!
//! The first built-in (`argon2id`) is pure over its `params` and touches
//! neither, so the placeholders are enough to land the execution model.

/// Access to the DBMS on the current transaction (placeholder — see module
/// docs).
#[derive(Debug, Clone, Default)]
pub struct FnCtx {
    _private: (),
}

impl FnCtx {
    /// Construct an empty context.
    pub fn new() -> Self {
        Self { _private: () }
    }
}

/// The batch the function executes within — read aliases, append ops
/// (placeholder — see module docs).
#[derive(Debug, Clone, Default)]
pub struct FnBatch {
    _private: (),
}

impl FnBatch {
    /// Construct an empty batch view.
    pub fn new() -> Self {
        Self { _private: () }
    }
}
