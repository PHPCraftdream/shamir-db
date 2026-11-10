use std::collections::BTreeMap;

use shamir_wasm_admission::AdmissionLimits;

/// Compiler capability stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    /// Pure scalar and validator compilation.
    #[default]
    Mvp,
    /// Procedure compilation, reserved for the later language stage.
    Procedures,
}

/// Lowerable compiler resource limits; hard ceilings are always applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompileLimits {
    pub max_source_bytes: usize,
    pub max_tokens: usize,
    pub max_ast_nodes: usize,
    pub max_ast_depth: usize,
    pub max_type_depth: usize,
    pub max_functions: usize,
    pub max_locals_per_function: usize,
    pub max_parameters: usize,
    pub max_record_fields: usize,
    pub max_single_literal_bytes: usize,
    pub max_total_literal_bytes: usize,
    pub max_static_bytes: usize,
    pub max_output_bytes: usize,
    pub max_runtime_bytes: usize,
    pub max_function_body_bytes: usize,
    /// Per-function IR node cap (`B001`); emitted Wasm operators are capped by
    /// `CompileOptions::admission` during self-admission (`B002`).
    pub max_ir_nodes_per_function: usize,
    pub max_ir_nodes: usize,
    pub max_call_chain: usize,
    pub compile_fuel: u64,
}

/// A zero-valued production limit rejected while normalizing options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimitError {
    /// Name of the invalid limit field.
    pub field: &'static str,
}

/// Front-end and admission options for compilation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileOptions {
    /// Capability stage.
    pub stage: Stage,
    /// Compiler limits, lowered to hard ceilings before use.
    pub limits: CompileLimits,
    /// WASM module admission limits retained for the compile pipeline.
    pub admission: AdmissionLimits,
    /// Previously emitted callable signatures indexed by arbitrary target name.
    pub callable_catalog: BTreeMap<String, Vec<u8>>,
}

impl CompileLimits {
    /// Clamps every field to its hard ceiling and rejects production zeroes.
    pub fn clamped(self) -> Result<Self, LimitError> {
        Ok(Self {
            max_source_bytes: clamp_usize(self.max_source_bytes, 1_048_576, "max_source_bytes")?,
            max_tokens: clamp_usize(self.max_tokens, 200_000, "max_tokens")?,
            max_ast_nodes: clamp_usize(self.max_ast_nodes, 200_000, "max_ast_nodes")?,
            max_ast_depth: clamp_usize(self.max_ast_depth, 128, "max_ast_depth")?,
            max_type_depth: clamp_usize(self.max_type_depth, 128, "max_type_depth")?,
            max_functions: clamp_usize(self.max_functions, 256, "max_functions")?,
            max_locals_per_function: clamp_usize(
                self.max_locals_per_function,
                1024,
                "max_locals_per_function",
            )?,
            max_parameters: clamp_usize(self.max_parameters, 32, "max_parameters")?,
            max_record_fields: clamp_usize(self.max_record_fields, 128, "max_record_fields")?,
            max_single_literal_bytes: clamp_usize(
                self.max_single_literal_bytes,
                65_536,
                "max_single_literal_bytes",
            )?,
            max_total_literal_bytes: clamp_usize(
                self.max_total_literal_bytes,
                4_194_304,
                "max_total_literal_bytes",
            )?,
            max_static_bytes: clamp_usize(self.max_static_bytes, 4_194_304, "max_static_bytes")?,
            max_output_bytes: clamp_usize(self.max_output_bytes, 8_388_608, "max_output_bytes")?,
            max_runtime_bytes: clamp_usize(self.max_runtime_bytes, 65_536, "max_runtime_bytes")?,
            max_function_body_bytes: clamp_usize(
                self.max_function_body_bytes,
                1_048_576,
                "max_function_body_bytes",
            )?,
            max_ir_nodes_per_function: clamp_usize(
                self.max_ir_nodes_per_function,
                500_000,
                "max_ir_nodes_per_function",
            )?,
            max_ir_nodes: clamp_usize(self.max_ir_nodes, 200_000, "max_ir_nodes")?,
            max_call_chain: clamp_usize(self.max_call_chain, 64, "max_call_chain")?,
            compile_fuel: clamp_u64(self.compile_fuel, 20_000_000, "compile_fuel")?,
        })
    }
}

impl Default for CompileLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 1_048_576,
            max_tokens: 200_000,
            max_ast_nodes: 200_000,
            max_ast_depth: 128,
            max_type_depth: 128,
            max_functions: 256,
            max_locals_per_function: 1024,
            max_parameters: 32,
            max_record_fields: 128,
            max_single_literal_bytes: 65_536,
            max_total_literal_bytes: 4_194_304,
            max_static_bytes: 4_194_304,
            max_output_bytes: 8_388_608,
            max_runtime_bytes: 65_536,
            max_function_body_bytes: 1_048_576,
            max_ir_nodes_per_function: 500_000,
            max_ir_nodes: 200_000,
            max_call_chain: 64,
            compile_fuel: 20_000_000,
        }
    }
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            stage: Stage::Mvp,
            limits: CompileLimits::default(),
            admission: AdmissionLimits::default(),
            callable_catalog: BTreeMap::new(),
        }
    }
}

fn clamp_usize(value: usize, ceiling: usize, field: &'static str) -> Result<usize, LimitError> {
    if value == 0 && !cfg!(test) {
        return Err(LimitError { field });
    }
    Ok(value.min(ceiling))
}

fn clamp_u64(value: u64, ceiling: u64, field: &'static str) -> Result<u64, LimitError> {
    if value == 0 && !cfg!(test) {
        return Err(LimitError { field });
    }
    Ok(value.min(ceiling))
}
