use std::time::Duration;

/// Slack reserved for the rest of a request frame (envelope, names, framing).
const FRAME_OVERHEAD_BYTES: u128 = 64 * 1024;

/// Caps enforced by [`AdmissionPolicy::Admission`](super::AdmissionPolicy).
///
/// Every cap is an inclusive upper bound: a value equal to the limit passes,
/// one above it is rejected with the variant naming that cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionLimits {
    /// Whole module size in bytes.
    pub max_module_bytes: usize,
    /// Defined functions (function section count).
    pub max_functions: usize,
    /// Entries of the type section.
    pub max_types: usize,
    /// Imports of all kinds.
    pub max_imports: usize,
    /// Exports of all kinds.
    pub max_exports: usize,
    /// Globals defined by the global section.
    pub max_globals: usize,
    /// Declared locals of one function (sum of the declaration counts).
    pub max_locals_per_function: usize,
    /// Encoded size of one function body in bytes.
    pub max_function_body_bytes: usize,
    /// Operators of one function body.
    pub max_operators_per_function: usize,
    /// Nesting depth of `block`/`loop`/`if` within one function.
    pub max_block_depth: usize,
    /// Targets of one `br_table` (default label excluded).
    pub max_br_table_targets: usize,
    /// Parameters plus results of one function type.
    pub max_params_and_results: usize,
    /// Declared initial or maximum size of the table, in elements.
    pub max_table_elements: usize,
    /// Element segments.
    pub max_element_segments: usize,
    /// Items summed over all element segments.
    pub max_element_items_total: usize,
    /// Data segments.
    pub max_data_segments: usize,
    /// Bytes summed over all data segments.
    pub max_data_bytes_total: usize,
    /// Declared initial memory size in 64 KiB pages.
    pub max_memory_min_pages: usize,
    /// Rust source size accepted by the compile path (carried here for the
    /// facade; not checked by the module scan).
    pub max_source_bytes: usize,
    /// Wall-clock cap on one admitted compile (scan plus compile); the job
    /// cannot be cancelled, see the compile gate. Not applied under the Load
    /// policy.
    pub max_compile_time: Duration,
    /// Compiles running at once across the engine; callers beyond it queue.
    pub max_concurrent_compiles: usize,
}

impl Default for AdmissionLimits {
    fn default() -> Self {
        Self {
            max_module_bytes: 8 * 1024 * 1024,
            max_functions: 20_000,
            max_types: 5_000,
            max_imports: 64,
            max_exports: 64,
            max_globals: 1_000,
            max_locals_per_function: 10_000,
            max_function_body_bytes: 1024 * 1024,
            max_operators_per_function: 500_000,
            max_block_depth: 256,
            max_br_table_targets: 4_096,
            max_params_and_results: 128,
            max_table_elements: 100_000,
            max_element_segments: 1_000,
            max_element_items_total: 200_000,
            max_data_segments: 10_000,
            max_data_bytes_total: 4 * 1024 * 1024,
            max_memory_min_pages: 1_024,
            max_source_bytes: 1024 * 1024,
            max_compile_time: Duration::from_secs(10),
            max_concurrent_compiles: std::thread::available_parallelism()
                .map_or(1, |cores| cores.get() / 2)
                .max(1),
        }
    }
}

impl AdmissionLimits {
    /// Checks that a maximal module (base64-encoded, 4/3 expansion) plus a
    /// maximal source plus framing overhead fits strictly under `frame_cap`.
    pub fn validate_against_frame(&self, frame_cap: usize) -> Result<(), String> {
        let module = self.max_module_bytes as u128;
        let encoded_module = (4 * module).div_ceil(3);
        let needed = encoded_module + self.max_source_bytes as u128 + FRAME_OVERHEAD_BYTES;
        if needed < frame_cap as u128 {
            Ok(())
        } else {
            Err(format!(
                "admission limits do not fit the frame cap: base64 module {encoded_module} \
                 (max_module_bytes {}) + max_source_bytes {} + {FRAME_OVERHEAD_BYTES} overhead \
                 = {needed} must be < frame cap {frame_cap}",
                self.max_module_bytes, self.max_source_bytes
            ))
        }
    }
}
