//! Bounded reason labels for `shamir_module_admission_rejected_total{reason}`.

use shamir_wasm_admission::{AdmissionError, AdmissionPolicy, ModuleReport};
use std::sync::atomic::{AtomicU64, Ordering};

pub const ADMISSION_REJECTION_REASONS: [&str; 7] = [
    "parse",
    "validation",
    "import",
    "operator",
    "abi",
    "interface",
    "limits",
];

static REJECTIONS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];

/// Admits a core WebAssembly module under `policy` in one `wasmparser` pass:
/// parse, feature-set validation, structural scan and (under
/// [`AdmissionPolicy::Admission`]) numeric caps plus the export contract.
///
/// Sections are validated before their caps are checked; function bodies are
/// scanned (caps, forbidden operators) before they are validated.
pub fn admit_module(
    bytes: &[u8],
    policy: &AdmissionPolicy,
) -> Result<ModuleReport, AdmissionError> {
    shamir_wasm_admission::admit_module(bytes, policy)
        .inspect_err(|err| note_rejected(classify(err)))
}

pub fn admission_rejection_totals() -> Vec<(&'static str, u64)> {
    ADMISSION_REJECTION_REASONS
        .iter()
        .zip(REJECTIONS.iter())
        .map(|(reason, counter)| (*reason, counter.load(Ordering::Relaxed)))
        .collect()
}

pub(crate) fn note_rejected(reason_index: usize) {
    if let Some(counter) = REJECTIONS.get(reason_index) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn classify(err: &AdmissionError) -> usize {
    match err {
        AdmissionError::Parse(_) | AdmissionError::ComponentEncoding => 0,
        AdmissionError::Validation(_) => 1,
        AdmissionError::UnsanctionedImport { .. } => 2,
        AdmissionError::ForbiddenOperator { .. } => 3,
        AdmissionError::DuplicateAbiSection
        | AdmissionError::AbiSectionLength { .. }
        | AdmissionError::UnsupportedAbiVersion { .. }
        | AdmissionError::OriginNotUtf8
        | AdmissionError::OriginTooLong { .. }
        | AdmissionError::DuplicateOriginSection => 4,
        AdmissionError::MissingExport { .. }
        | AdmissionError::WrongExportKind { .. }
        | AdmissionError::WrongExportType { .. }
        | AdmissionError::WrongImportType { .. }
        | AdmissionError::TooManyTables { .. }
        | AdmissionError::NonFuncrefTable
        | AdmissionError::TableTooLarge { .. }
        | AdmissionError::TooManyElementSegments { .. }
        | AdmissionError::TooManyElementItems { .. }
        | AdmissionError::TooManyDataSegments { .. }
        | AdmissionError::DataTooLarge { .. }
        | AdmissionError::MemoryTooLarge { .. } => 5,
        AdmissionError::ModuleTooLarge { .. }
        | AdmissionError::TooManyTypes { .. }
        | AdmissionError::TooManyFunctions { .. }
        | AdmissionError::TooManyImports { .. }
        | AdmissionError::TooManyExports { .. }
        | AdmissionError::TooManyGlobals { .. }
        | AdmissionError::TooManyLocals { .. }
        | AdmissionError::FunctionBodyTooLarge { .. }
        | AdmissionError::TooManyOperators { .. }
        | AdmissionError::BlockDepthExceeded { .. }
        | AdmissionError::BrTableTooLarge { .. }
        | AdmissionError::TooManyParamsAndResults { .. } => 6,
    }
}
