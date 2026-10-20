//! Artifact origin tagging for the function/validator catalogue.
//!
//! Phase 0 of the native ↔ WASM parity campaign: a purely-additive field that
//! lets later phases branch on whether a catalogue row backs a WASM module or
//! a native Rust type. See
//! `docs/dev-artifacts/design/native-wasm-parity-phase0-findings.md` for the full seam map.
//!
//! Migration safety: catalogue rows persisted before this field existed have
//! no `kind` key. [`ArtifactKind::from_record`] treats any missing or
//! unrecognised value as [`ArtifactKind::Wasm`], so the entire existing
//! catalogue migrates transparently and existing WASM artifacts keep working
//! unchanged. Nothing reads `kind` to branch yet — this is plumbing only.

use shamir_types::types::value::QueryValue;

/// Catalogue field name used to persist the artifact origin.
pub const KIND_FIELD: &str = "kind";

/// Origin of a function or validator catalogue row.
///
/// Order matters for the `Default` impl: `Wasm` is the historical default and
/// must remain so to keep pre-existing catalogue rows round-tripping
/// correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ArtifactKind {
    /// Backed by a compiled WASM module (`wasm_b64` field carries the bytes).
    /// Default for any row that does not explicitly record a `kind`.
    #[default]
    Wasm,
    /// Backed by an in-process Rust type implementing `ShamirFunction`.
    /// Phase ≥1 rows; no `wasm_b64` payload is required for these.
    Native,
    /// Backed by a declarative schema (Phase A). The validator is compiled
    /// from field rules stored in the table catalogue — no user code, no
    /// WASM. The `kind` field in the validator catalogue carries
    /// `"declarative"` so the boot-pass can distinguish it from code
    /// validators and skip WASM materialisation.
    Declarative,
}

impl ArtifactKind {
    /// The string spelling persisted into the catalogue row's `kind` field.
    /// Stable — do not rename without a migration step.
    pub const fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Wasm => "wasm",
            ArtifactKind::Native => "native",
            ArtifactKind::Declarative => "declarative",
        }
    }

    /// Decode an `ArtifactKind` from its persisted string spelling.
    /// Unknown spellings map to [`ArtifactKind::Wasm`] (the historical
    /// default) rather than erroring, so a forward-incompatible catalogue
    /// row degrades gracefully instead of failing boot.
    pub fn parse_kind(s: &str) -> Self {
        match s {
            "native" => ArtifactKind::Native,
            "declarative" => ArtifactKind::Declarative,
            // "wasm" and any unrecognised value — fail-safe to the historical
            // default. The boot path already logs-and-skips rows it cannot
            // materialise; an unknown kind is not by itself a skip condition.
            _ => ArtifactKind::Wasm,
        }
    }

    /// Render as a [`QueryValue::Str`] for injection into a catalogue row.
    pub fn as_query_value(self) -> QueryValue {
        QueryValue::Str(self.as_str().to_string())
    }

    /// Read `kind` off a persisted catalogue record (a `QueryValue::Map`),
    /// defaulting to [`ArtifactKind::Wasm`] when the field is absent, `Null`,
    /// or holds an unrecognised string.
    ///
    /// This is the migration-safety boundary: pre-Phase-0 rows have no `kind`
    /// field and must continue to load as WASM. Callers in the boot path and
    /// later phases should always go through this helper rather than reading
    /// the field directly.
    pub fn from_record(record: &QueryValue) -> Self {
        match record.get(KIND_FIELD) {
            Some(QueryValue::Str(s)) => ArtifactKind::parse_kind(s),
            // Absent, Null, or wrong type → historical default.
            _ => ArtifactKind::Wasm,
        }
    }
}

impl std::fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
