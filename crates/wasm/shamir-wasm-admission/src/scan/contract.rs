use crate::abi::{EXPORT_ALLOC, EXPORT_CALL, EXPORT_MEMORY};
use crate::types::AdmissionError;
use wasmparser::{ExternalKind, FuncType, ValType};

/// One export as `(name, kind, index)`.
pub(super) type ExportEntry = (String, ExternalKind, u32);

fn kind_name(kind: ExternalKind) -> &'static str {
    match kind {
        ExternalKind::Func | ExternalKind::FuncExact => "func",
        ExternalKind::Table => "table",
        ExternalKind::Memory => "memory",
        ExternalKind::Global => "global",
        ExternalKind::Tag => "tag",
    }
}

fn render_types(types: &[ValType]) -> String {
    types
        .iter()
        .map(ValType::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_signature(ty: &FuncType) -> String {
    let results = ty.results();
    let rendered = render_types(results);
    match results {
        [_] => format!("({}) -> {rendered}", render_types(ty.params())),
        _ => format!("({}) -> ({rendered})", render_types(ty.params())),
    }
}

fn find<'e>(exports: &'e [ExportEntry], name: &str) -> Option<&'e ExportEntry> {
    exports.iter().find(|(n, _, _)| n == name)
}

fn check_func_export(
    exports: &[ExportEntry],
    func_type_indices: &[u32],
    types: &[FuncType],
    name: &'static str,
    (params, results): (&[ValType], &[ValType]),
    expected: &'static str,
) -> Result<(), AdmissionError> {
    let &(_, kind, index) = find(exports, name).ok_or(AdmissionError::MissingExport { name })?;
    if !matches!(kind, ExternalKind::Func | ExternalKind::FuncExact) {
        return Err(AdmissionError::WrongExportKind {
            name,
            expected: "func",
            found: kind_name(kind).to_string(),
        });
    }
    let ty = usize::try_from(index)
        .ok()
        .and_then(|i| func_type_indices.get(i))
        .and_then(|&t| usize::try_from(t).ok())
        .and_then(|t| types.get(t))
        .ok_or_else(|| {
            AdmissionError::Validation(format!("export `{name}` refers to an unknown function"))
        })?;
    if ty.params() != params || ty.results() != results {
        return Err(AdmissionError::WrongExportType {
            name,
            expected,
            found: render_signature(ty),
        });
    }
    Ok(())
}

/// Checks the guest entrypoint contract: `memory`, `shamir_alloc: (i32) -> i32`
/// and `shamir_call: (i32, i32) -> i64`, in that order.
pub(super) fn check_contract(
    exports: &[ExportEntry],
    func_type_indices: &[u32],
    types: &[FuncType],
) -> Result<(), AdmissionError> {
    let &(_, kind, _) = find(exports, EXPORT_MEMORY).ok_or(AdmissionError::MissingExport {
        name: EXPORT_MEMORY,
    })?;
    if kind != ExternalKind::Memory {
        return Err(AdmissionError::WrongExportKind {
            name: EXPORT_MEMORY,
            expected: "memory",
            found: kind_name(kind).to_string(),
        });
    }
    check_func_export(
        exports,
        func_type_indices,
        types,
        EXPORT_ALLOC,
        (&[ValType::I32], &[ValType::I32]),
        "(i32) -> i32",
    )?;
    check_func_export(
        exports,
        func_type_indices,
        types,
        EXPORT_CALL,
        (&[ValType::I32, ValType::I32], &[ValType::I64]),
        "(i32, i32) -> i64",
    )
}
