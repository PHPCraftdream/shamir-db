use shamir_wasm_admission::{admit_module, AdmissionPolicy, ModuleReport};

use crate::back::ir::{verify, IrError};
use crate::back::lower::lower;
use crate::back::{emit_compiled_module, EmitError};
use crate::front::budget::CompileBudget;
use crate::front::{self, EntryKind};
use crate::sem::{self, TypeData, TypeId, TypeInterner};

use super::error::{limit_error, CompileError};
use super::options::{CompileOptions, Stage};
use super::{Diagnostic, Span};

/// A compiled, self-admitted module (design section 6).
#[derive(Clone, Debug)]
pub struct CompiledModule {
    /// The admitted WebAssembly bytes.
    pub wasm: Vec<u8>,
    /// The admission scan report produced for `wasm`.
    pub report: ModuleReport,
    /// Canonical UTF-8 procedure signature (`procedure(p:T,...)->R`); absent
    /// for scalar and validator entries. Record fields are UTF-8-byte sorted.
    pub signature: Option<Vec<u8>>,
}

/// Compiles one source module into a self-admitted WebAssembly artifact and
/// compiler-owned callable signature metadata, when the entry is a procedure.
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
    // finding 19: the scan runs under the shared pipeline budget (parse_scoped
    // re-scans through the same budget, so scan work is charged exactly once
    // per pass it performs).
    front::scan_with_budget(source, &normalized, &mut budget)
        .map_err(|diagnostic| CompileError::Diagnostics(vec![diagnostic]))?;
    let module = front::parse_scoped(source, &normalized, &mut budget)
        .map_err(|diagnostic| CompileError::Diagnostics(vec![diagnostic]))?;
    let typed = sem::check_scoped_with_catalog(
        &module,
        &normalized,
        options.stage,
        &options.callable_catalog,
        &mut budget,
    )
    .map_err(CompileError::Diagnostics)?;
    // Entry gate: scalar and validator entries have wrappers in this build;
    // procedure entries compile only at `Stage::Procedures` (design section 3;
    // the checker already rejects them at `Stage::Mvp`).
    let procedure_allowed =
        typed.entry_kind == EntryKind::Procedure && options.stage == Stage::Procedures;
    if typed.entry_kind != EntryKind::Scalar
        && typed.entry_kind != EntryKind::Validator
        && !procedure_allowed
    {
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
    let wasm =
        emit_compiled_module(&ir, &normalized, &mut budget).map_err(|error| match error {
            EmitError::Runtime(error) => {
                CompileError::InternalInvariant(format!("emission: {error}"))
            }
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
    // finding 19: admission precharge — the emitted module length also bills
    // the single pipeline budget, so total work stays under one fuel ceiling.
    if let Err(diagnostic) = budget.charge(observed_len, typed.span) {
        return Err(CompileError::LimitExceeded {
            code: "B001",
            span: diagnostic.primary,
            limit: budget.limit(),
            observed: budget.spent(),
        });
    }
    let report = admit_module(&wasm, &AdmissionPolicy::Admission(options.admission))
        .map_err(CompileError::Admission)?;
    let signature = if typed.entry_kind == EntryKind::Procedure {
        Some(encode_procedure_signature(&typed).ok_or_else(|| {
            CompileError::InternalInvariant(
                "procedure entry signature contains an unsupported type".to_string(),
            )
        })?)
    } else {
        None
    };
    Ok(CompiledModule {
        wasm,
        report,
        signature,
    })
}

/// Encodes the entry's structural signature; the source function name is absent.
fn encode_procedure_signature(typed: &sem::TypedModule) -> Option<Vec<u8>> {
    if typed.entry_kind != EntryKind::Procedure {
        return None;
    }
    let entry = typed.function(typed.entry)?;
    // Type names, record fields, parameters and punctuation are source-bounded;
    // reserve only a modest prefix and grow within existing compiler limits.
    let mut signature = Vec::with_capacity(32);
    signature.extend_from_slice(b"procedure(");
    for (index, parameter) in entry.parameters.iter().enumerate() {
        if index != 0 {
            signature.push(b',');
        }
        signature.extend_from_slice(parameter.name.as_bytes());
        signature.push(b':');
        append_signature_type(&typed.types, parameter.ty, 0, &mut signature)?;
    }
    signature.extend_from_slice(b")->");
    append_signature_type(&typed.types, entry.return_type, 0, &mut signature)?;
    Some(signature)
}

/// Writes one canonical wire type into the signature buffer.
fn append_signature_type(
    types: &TypeInterner,
    ty: TypeId,
    depth: u32,
    output: &mut Vec<u8>,
) -> Option<()> {
    if depth > 128 {
        return None;
    }
    match types.data(ty)? {
        TypeData::I32 => output.extend_from_slice(b"i32"),
        TypeData::I64 => output.extend_from_slice(b"i64"),
        TypeData::F64 => output.extend_from_slice(b"f64"),
        TypeData::Bool => output.extend_from_slice(b"bool"),
        TypeData::String => output.extend_from_slice(b"string"),
        TypeData::Bytes => output.extend_from_slice(b"bytes"),
        TypeData::Null => output.extend_from_slice(b"null"),
        TypeData::Optional(inner) => {
            append_signature_type(types, inner, depth + 1, output)?;
            output.push(b'?');
        }
        TypeData::List(inner) => {
            output.push(b'[');
            append_signature_type(types, inner, depth + 1, output)?;
            output.push(b']');
        }
        TypeData::Record(index) => {
            let shape = types.record(index)?;
            output.push(b'{');
            for (field_index, (name, field_type)) in shape.fields.iter().enumerate() {
                if field_index != 0 {
                    output.push(b',');
                }
                output.extend_from_slice(name.as_bytes());
                output.push(b':');
                append_signature_type(types, *field_type, depth + 1, output)?;
            }
            output.push(b'}');
        }
        TypeData::Validation
        | TypeData::Batch
        | TypeData::Request
        | TypeData::Response
        | TypeData::Error => return None,
    }
    Some(())
}
