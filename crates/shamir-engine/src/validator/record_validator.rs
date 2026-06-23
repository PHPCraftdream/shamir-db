//! The narrow `RecordValidator` role — a validator is NOT a general-purpose
//! `ShamirFunction`.
//!
//! `RecordValidator` receives field access by name via `&dyn RecordFields`
//! and a lean [`ValidatorCtx`]; it returns a [`Validation`].  Interning is
//! hidden from the validator author.

use std::sync::Arc;

use async_trait::async_trait;
use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_types::access::Actor;
use shamir_types::core::interner::{Interner, InternerKey};

use super::{record_fields::RecordFields, Validation};

// ── ValidatorCtx ──────────────────────────────────────────────────────────────

/// Narrow context passed to every [`RecordValidator::validate`] call.
///
/// - `actor` — the identity that initiated the write.
/// - [`field_name`](Self::field_name) — the ONLY interner capability exposed:
///   de-intern a field id back to its name for an error message. The interner
///   itself is held privately so declarative / user validators cannot iterate
///   keys, intern new names, or otherwise reach the full [`Interner`] surface.
/// - [`scalars`](Self::scalars) — optional [`ScalarResolver`] for Phase B
///   scalar-bridge rules.  When `None`, scalar-bridge rules are silently
///   skipped (the validator does not panic).
///
/// A `db` handle (tx-scoped read-only snapshot) for relational checks is
/// reserved for Phase C and is not part of this struct yet.
pub struct ValidatorCtx<'a> {
    /// Who initiated the write.
    pub actor: &'a Actor,
    /// Repo interner — **private**. Reached only through
    /// [`field_name`](Self::field_name); never exposed wholesale to validators.
    interner: &'a Interner,
    /// Optional scalar resolver (Phase B scalar-bridge).  `None` on paths
    /// where no resolver is wired (e.g. tests, or tables without a user
    /// scalar layer) — scalar-bridge rules silently skip in that case.
    scalars: Option<&'a ScalarResolver>,
}

impl<'a> ValidatorCtx<'a> {
    /// Construct a validator context from the actor and the repo interner,
    /// with **no** scalar resolver (scalar-bridge rules will be skipped).
    pub fn new(actor: &'a Actor, interner: &'a Interner) -> Self {
        Self {
            actor,
            interner,
            scalars: None,
        }
    }

    /// Construct a validator context with a scalar resolver attached.
    ///
    /// This is the Phase B entry point: scalar-bridge rules in a
    /// [`SchemaValidator`](super::schema::SchemaValidator) resolve their
    /// named scalar through this resolver.
    pub fn with_scalars(
        actor: &'a Actor,
        interner: &'a Interner,
        scalars: &'a ScalarResolver,
    ) -> Self {
        Self {
            actor,
            interner,
            scalars: Some(scalars),
        }
    }

    /// De-intern a field id back to its name, for error-message construction.
    ///
    /// This is the only interner capability a validator gets: it cannot iterate
    /// keys, intern new names, or otherwise touch the full [`Interner`].
    pub fn field_name(&self, id: &InternerKey) -> Option<Arc<str>> {
        self.interner.get_str(id)
    }

    /// The scalar resolver, if one was attached via [`with_scalars`](Self::with_scalars).
    ///
    /// Returns `None` on paths where no resolver is wired; scalar-bridge
    /// rules must treat `None` as "skip silently" (never panic).
    pub fn scalars(&self) -> Option<&ScalarResolver> {
        self.scalars
    }
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
