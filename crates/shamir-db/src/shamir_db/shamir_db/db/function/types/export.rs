/// The stored artifact of a function, as persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionExport {
    /// Exact stored module bytes.
    pub bytes: Vec<u8>,
    /// Source text for `lang = rust` functions.
    pub source: Option<String>,
    /// Composite module hash in 32 hex chars.
    pub hash: String,
}
