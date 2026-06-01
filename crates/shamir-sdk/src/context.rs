//! Placeholder execution-context handles (slice 3).
//!
//! [`Ctx`] and [`Batch`] are empty this slice. Real bodies arrive in slice 4
//! (DBMS handle, transaction snapshot, batch staging buffer).

/// Access to the DBMS on the current transaction (placeholder).
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    _private: (),
}

impl Ctx {
    /// Construct an empty context.
    pub fn new() -> Self {
        Self { _private: () }
    }
}

/// The batch the function executes within (placeholder).
#[derive(Debug, Clone, Default)]
pub struct Batch {
    _private: (),
}

impl Batch {
    /// Construct an empty batch view.
    pub fn new() -> Self {
        Self { _private: () }
    }
}
