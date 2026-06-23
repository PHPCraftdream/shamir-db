//! The narrow `RecordValidator` role — a validator is NOT a general-purpose
//! `ShamirFunction`.
//!
//! `RecordValidator` receives field access by name via `&dyn RecordFields`
//! and a lean [`ValidatorCtx`]; it returns a [`Validation`].  Interning is
//! hidden from the validator author.

use async_trait::async_trait;
use shamir_types::access::Actor;
use shamir_types::core::interner::Interner;

use super::{record_fields::RecordFields, Validation};

// ── ValidatorCtx ──────────────────────────────────────────────────────────────

/// Narrow context passed to every [`RecordValidator::validate`] call.
///
/// - `actor` — the identity that initiated the write.
/// - `interner` — the repo interner, available so a validator can de-intern
///   a field name back to a string when constructing error messages.
///   **Not** intended for general key iteration; use it only to format errors.
/// - `db` — reserved for Phase C (relational checks / FK lookups); `None`
///   in Phase 0.
pub struct ValidatorCtx<'a> {
    /// Who initiated the write.
    pub actor: &'a Actor,
    /// Repo interner — for de-interning field names in error messages.
    pub interner: &'a Interner,
}

// ── RecordValidator trait ─────────────────────────────────────────────────────

/// A validator in the narrow role: `(new_record, old_record, ctx) → Validation`.
///
/// Implementors must be `Send + Sync` (validators live in an `Arc` in the
/// registry and are called concurrently from multiple write paths).
///
/// The method is `async` to support WASM validators (which require an async
/// guest call).  Native validators simply return immediately.
///
/// # Implementing
///
/// Use [`super::NativeRecordValidator`] for Rust closures and
/// [`super::WasmRecordValidator`] for WASM guests.  Declarative
/// `SchemaValidator` will implement this trait in Phase A.
#[async_trait]
pub trait RecordValidator: Send + Sync {
    /// Validate a write operation.
    ///
    /// - `new` — the record being written (always `Some` for
    ///   INSERT/UPDATE/UPSERT; `None` for DELETE where only `old` is set).
    /// - `old` — the previous record (for UPDATE/UPSERT/DELETE; `None` for
    ///   pure INSERT).
    /// - `ctx` — actor + interner for error construction.
    async fn validate(
        &self,
        new: Option<&dyn RecordFields>,
        old: Option<&dyn RecordFields>,
        ctx: &ValidatorCtx<'_>,
    ) -> Validation;
}
