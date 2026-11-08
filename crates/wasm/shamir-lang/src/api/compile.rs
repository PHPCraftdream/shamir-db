use shamir_wasm_admission::{admit_module, AdmissionPolicy, ModuleReport};

use crate::back::ir::{verify, IrError};
use crate::back::lower::lower;
use crate::back::{emit_compiled_module, EmitError};
use crate::front::budget::CompileBudget;
use crate::front::{self, EntryKind};
use crate::sem;

use super::error::{limit_error, CompileError};
use super::options::CompileOptions;
use super::{Diagnostic, Span};

/// A compiled, self-admitted module (design section 6).
#[derive(Clone, Debug)]
pub struct CompiledModule {
    /// The admitted WebAssembly bytes.
    pub wasm: Vec<u8>,
    /// The admission scan report produced for `wasm`.
    pub report: ModuleReport,
}

/// Compiles one source module into a self-admitted WebAssembly artifact:
/// scan, parse, check, lower, verify, emit, then admit the emitted bytes.
pub fn compile(source: &str, options: &CompileOptions) -> Result<CompiledModule, CompileError> {
    let normalized = options.limits.clamped().map_err(|error| {
        CompileError::Diagnostics(vec![Diagnostic::new(
            "B001",
            format!("invalid zero compiler limit: {}", error.field),
            Span::source_start(),
        )])
    })?;
    let observed = u64::try_from(source.len()).unwrap_or(u64::MAX);
    let source_limit = normalized.max_source_bytes as u64;
    if observed > source_limit {
        return Err(limit_error(Span::source_start(), source_limit, observed));
    }
    let mut budget = CompileBudget::new(&normalized).map_err(|error| {
        CompileError::Diagnostics(vec![Diagnostic::new(
            "B001",
            format!("invalid zero compiler limit: {}", error.field),
            Span::source_start(),
        )])
    })?;
    front::scan(source, &normalized)
        .map_err(|diagnostic| CompileError::Diagnostics(vec![diagnostic]))?;
    let module = front::parse(source, &normalized)
        .map_err(|diagnostic| CompileError::Diagnostics(vec![diagnostic]))?;
    let typed =
        sem::check(&module, &normalized, options.stage).map_err(CompileError::Diagnostics)?;
    // Entry gate: only the scalar facade has a wrapper in this build.
    if typed.entry_kind != EntryKind::Scalar {
        let span = typed
            .function(typed.entry)
            .map(|function| function.span)
            .unwrap_or(typed.span);
        return Err(CompileError::Diagnostics(vec![Diagnostic::new(
            "T007",
            "entry kind not supported by this compiler build yet".to_string(),
            span,
        )]));
    }
    let ir = lower(&module, &typed, &normalized, &mut budget).map_err(|error| match error {
        IrError::LimitExceeded {
            code,
            span,
            limit,
            observed,
        } => CompileError::LimitExceeded {
            code,
            span,
            limit,
            observed,
        },
        IrError::Internal(message) => CompileError::InternalInvariant(message.to_string()),
    })?;
    verify(&ir)
        .map_err(|error| CompileError::InternalInvariant(format!("ir verifier: {error}")))?;
    let wasm = emit_compiled_module(&ir, &normalized).map_err(|error| match error {
        EmitError::Runtime(error) => CompileError::InternalInvariant(format!("emission: {error}")),
        EmitError::HelperOrder(error) => {
            CompileError::InternalInvariant(format!("helper order: {error}"))
        }
        EmitError::LimitExceeded {
            code,
            limit,
            observed,
        } => CompileError::LimitExceeded {
            code,
            span: typed.span,
            limit,
            observed,
        },
        EmitError::Internal(message) => CompileError::InternalInvariant(message.to_string()),
    })?;
    let observed_len = u64::try_from(wasm.len()).unwrap_or(u64::MAX);
    let output_limit = normalized.max_output_bytes as u64;
    if observed_len > output_limit {
        return Err(limit_error(typed.span, output_limit, observed_len));
    }
    let report = admit_module(&wasm, &AdmissionPolicy::Admission(options.admission))
        .map_err(CompileError::Admission)?;
    Ok(CompiledModule { wasm, report })
}
