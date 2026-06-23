//! Native (Rust closure) implementation of [`RecordValidator`].
//!
//! Unlike the legacy [`NativeValidatorAdapter`](super::NativeValidatorAdapter)
//! which threaded the record through `QueryValue` and `ShamirFunction`, this
//! implementation receives `&dyn RecordFields` directly — by name, zero-copy
//! for scalar checks.

use async_trait::async_trait;

use super::{
    record_fields::RecordFields,
    record_validator::{RecordValidator, ValidatorCtx},
    Validation,
};

/// Type alias for native validator closures.
///
/// The closure receives:
/// - `new` — the record being written (by-name field access, no interning).
/// - `old` — the previous record if any.
/// - `ctx` — actor + interner (for error messages).
pub type NativeValidatorFnNew = dyn Fn(Option<&dyn RecordFields>, Option<&dyn RecordFields>, &ValidatorCtx<'_>) -> Validation
    + Send
    + Sync;

/// Wraps a native Rust closure as a [`RecordValidator`].
pub struct NativeRecordValidator {
    inner: Box<NativeValidatorFnNew>,
}

impl NativeRecordValidator {
    /// Create from any closure matching the native validator signature.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(
                Option<&dyn RecordFields>,
                Option<&dyn RecordFields>,
                &ValidatorCtx<'_>,
            ) -> Validation
            + Send
            + Sync
            + 'static,
    {
        Self { inner: Box::new(f) }
    }
}

#[async_trait]
impl RecordValidator for NativeRecordValidator {
    async fn validate(
        &self,
        new: Option<&dyn RecordFields>,
        old: Option<&dyn RecordFields>,
        ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        (self.inner)(new, old, ctx)
    }
}
