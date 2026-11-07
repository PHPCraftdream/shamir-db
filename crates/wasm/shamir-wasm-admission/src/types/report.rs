use super::module_hash::ModuleHash;

/// What the admission scan observed about an accepted module.
///
/// Counts are `u64`, byte sizes `usize`. Counters are filled under every
/// policy, caps or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleReport {
    /// Total module size in bytes.
    pub byte_size: usize,
    /// Defined functions.
    pub function_count: u64,
    /// Type section entries.
    pub type_count: u64,
    /// Imports of all kinds.
    pub import_count: u64,
    /// Exports of all kinds.
    pub export_count: u64,
    /// Defined globals.
    pub global_count: u64,
    /// Defined tables.
    pub table_count: u64,
    /// Element segments.
    pub element_segment_count: u64,
    /// Items summed over all element segments.
    pub element_item_count: u64,
    /// Data segments.
    pub data_segment_count: u64,
    /// Bytes summed over all data segments.
    pub data_bytes_total: u64,
    /// Declared initial pages of the defined memory (0 when none).
    pub memory_min_pages: u64,
    /// Largest encoded function body in bytes.
    pub largest_function_body_bytes: usize,
    /// Most operators in any one function.
    pub max_operators_in_function: u64,
    /// Deepest `block`/`loop`/`if` nesting seen in any function.
    pub max_block_depth_seen: u64,
    /// ABI version from `shamir.abi` (1 when the section is absent).
    pub abi_version: u32,
    /// Raw `shamir.origin` claim, unverified.
    pub origin_claim: Option<String>,
    /// Identity/corruption hash of the module bytes.
    pub hash: ModuleHash,
}
