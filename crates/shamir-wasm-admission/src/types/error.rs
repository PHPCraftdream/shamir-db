use crate::runtime::error::FunctionError;
use thiserror::Error;

/// Why a module was rejected; one variant per rejection class.
///
/// Numeric-cap variants carry `limit` and `observed` as `u64`.
#[derive(Debug, Error)]
pub enum AdmissionError {
    #[error("wasm sanitizer: parse error: {0}")]
    Parse(String),
    #[error(
        "wasm sanitizer: component-encoded artifact rejected — only core WebAssembly modules \
         are permitted"
    )]
    ComponentEncoding,
    #[error("wasm admission: module validation failed: {0}")]
    Validation(String),
    #[error(
        "wasm sanitizer: unsanctioned import `{module}::{name}` ({kind}) — only the \
         shamir_host host ABI is permitted"
    )]
    UnsanctionedImport {
        module: String,
        name: String,
        kind: &'static str,
    },
    #[error("wasm admission: forbidden operator `{operator}` in function {function_index}")]
    ForbiddenOperator {
        function_index: u32,
        operator: &'static str,
    },
    #[error("wasm admission: too many tables: {observed} (at most 1 is permitted)")]
    TooManyTables { observed: u64 },
    #[error("wasm admission: only funcref tables are permitted")]
    NonFuncrefTable,

    #[error("wasm admission: duplicate `shamir.abi` section")]
    DuplicateAbiSection,
    #[error("wasm admission: `shamir.abi` section must be exactly 4 bytes, got {observed}")]
    AbiSectionLength { observed: usize },
    #[error(
        "wasm admission: unsupported ABI version {found} in `shamir.abi` (supported: {supported})"
    )]
    UnsupportedAbiVersion { found: u32, supported: u32 },
    #[error("wasm admission: `shamir.origin` section is not valid UTF-8")]
    OriginNotUtf8,
    #[error("wasm admission: `shamir.origin` section too long: {observed} bytes (limit {limit})")]
    OriginTooLong { limit: usize, observed: usize },
    #[error("wasm admission: duplicate `shamir.origin` section")]
    DuplicateOriginSection,

    #[error("wasm admission: missing required export `{name}`")]
    MissingExport { name: &'static str },
    #[error("wasm admission: export `{name}` must be a {expected}, found {found}")]
    WrongExportKind {
        name: &'static str,
        expected: &'static str,
        found: String,
    },
    #[error("wasm admission: export `{name}` must have type {expected}, found {found}")]
    WrongExportType {
        name: &'static str,
        expected: &'static str,
        found: String,
    },

    #[error("wasm admission: module too large: {observed} bytes (limit {limit})")]
    ModuleTooLarge { limit: u64, observed: u64 },
    #[error("wasm admission: too many types: {observed} (limit {limit})")]
    TooManyTypes { limit: u64, observed: u64 },
    #[error("wasm admission: too many functions: {observed} (limit {limit})")]
    TooManyFunctions { limit: u64, observed: u64 },
    #[error("wasm admission: too many imports: {observed} (limit {limit})")]
    TooManyImports { limit: u64, observed: u64 },
    #[error("wasm admission: too many exports: {observed} (limit {limit})")]
    TooManyExports { limit: u64, observed: u64 },
    #[error("wasm admission: too many globals: {observed} (limit {limit})")]
    TooManyGlobals { limit: u64, observed: u64 },
    #[error(
        "wasm admission: too many locals in function {function_index}: {observed} (limit {limit})"
    )]
    TooManyLocals {
        function_index: u32,
        limit: u64,
        observed: u64,
    },
    #[error(
        "wasm admission: function {function_index} body too large: {observed} bytes (limit {limit})"
    )]
    FunctionBodyTooLarge {
        function_index: u32,
        limit: u64,
        observed: u64,
    },
    #[error(
        "wasm admission: too many operators in function {function_index}: {observed} (limit {limit})"
    )]
    TooManyOperators {
        function_index: u32,
        limit: u64,
        observed: u64,
    },
    #[error(
        "wasm admission: block depth exceeded in function {function_index}: {observed} (limit {limit})"
    )]
    BlockDepthExceeded {
        function_index: u32,
        limit: u64,
        observed: u64,
    },
    #[error(
        "wasm admission: br_table too large in function {function_index}: {observed} targets (limit {limit})"
    )]
    BrTableTooLarge {
        function_index: u32,
        limit: u64,
        observed: u64,
    },
    #[error(
        "wasm admission: too many params and results in type {type_index}: {observed} (limit {limit})"
    )]
    TooManyParamsAndResults {
        type_index: u32,
        limit: u64,
        observed: u64,
    },
    #[error("wasm admission: table too large: {observed} elements (limit {limit})")]
    TableTooLarge { limit: u64, observed: u64 },
    #[error("wasm admission: too many element segments: {observed} (limit {limit})")]
    TooManyElementSegments { limit: u64, observed: u64 },
    #[error("wasm admission: too many element items: {observed} (limit {limit})")]
    TooManyElementItems { limit: u64, observed: u64 },
    #[error("wasm admission: too many data segments: {observed} (limit {limit})")]
    TooManyDataSegments { limit: u64, observed: u64 },
    #[error("wasm admission: data segments too large: {observed} bytes (limit {limit})")]
    DataTooLarge { limit: u64, observed: u64 },
    #[error("wasm admission: memory minimum too large: {observed} pages (limit {limit})")]
    MemoryTooLarge { limit: u64, observed: u64 },
}

impl From<AdmissionError> for FunctionError {
    fn from(err: AdmissionError) -> Self {
        FunctionError::Compute(err.to_string())
    }
}
