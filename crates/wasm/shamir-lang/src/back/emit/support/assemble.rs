//! Section assembly of one prepared module image (types, imports, functions,
//! memory, globals, exports, code and the static data segment). Split out of
//! `emit/module.rs` so the module planner stays under its size budget.

use wasm_encoder::{
    CodeSection, ConstExpr, CustomSection, DataSection, Encode, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, GlobalSection, GlobalType, ImportSection,
    MemorySection, MemoryType, Module, TypeSection, ValType,
};

use shamir_wasm_admission::{
    ABI_SECTION, EXPORT_ALLOC, EXPORT_CALL, EXPORT_MEMORY, ORIGIN_SECTION, SUPPORTED_ABI_VERSION,
};

use crate::back::emit::module::EmitError;
use crate::back::ir::ValueKind;
use crate::back::runtime::{Layout, RuntimeError, STATIC_START};

/// Design hard ceiling on function types.
const MAX_TYPES: u64 = 512;

/// Runtime type indices the assembly refers to (positions in the fixed
/// runtime type list below; mirrors the planner's constants).
const TYPE_VOID_THREE_I32: u32 = 4;
const TYPE_ALLOC: u32 = 7;
const TYPE_CALL: u32 = 8;

/// One fully prepared module ready for section assembly.
pub(crate) struct ModuleImage {
    pub(crate) static_bytes: Vec<u8>,
    pub(crate) helper_functions: Vec<(u32, Function)>,
    pub(crate) error_raiser: Function,
    pub(crate) allocator: Function,
    pub(crate) wrapper: Function,
    pub(crate) user_functions: Vec<(u32, Function)>,
    pub(crate) eq_functions: Vec<(u32, Function)>,
    /// Generated projection helpers, appended after the eq helpers.
    pub(crate) projection_functions: Vec<(u32, Function)>,
    pub(crate) alloc_index: u32,
    pub(crate) call_index: u32,
    /// Extra function types beyond the ten runtime ones (user + eq).
    pub(crate) extra_types: Vec<(Vec<ValType>, Option<ValType>)>,
    pub(crate) heap_base: u64,
    pub(crate) layout: Layout,
    /// Stage-2 host imports after `raise_error`: `(import name, type index)`
    /// in operation order; empty for scalar and validator modules.
    pub(crate) db_imports: Vec<(&'static str, u32)>,
    /// Decode-class projection helpers of the database results, after the
    /// entry projection helpers.
    pub(crate) db_projection_functions: Vec<(u32, Function)>,
    /// Database call helpers, last in the function index space.
    pub(crate) db_call_functions: Vec<(u32, Function)>,
}

/// Assembles the complete module: custom ABI/origin sections, type, import
/// (error raiser at index 0), function, memory, global, export and code
/// sections, plus one active data segment carrying the static bytes. Returns
/// the module bytes and the encoded code-section length.
pub(crate) fn assemble(image: &ModuleImage) -> Result<(Vec<u8>, u64), RuntimeError> {
    let mut types = TypeSection::new();
    let i = ValType::I32;
    let l = ValType::I64;
    for (params, result) in [
        (vec![i, i, i], vec![i]),
        (vec![i, i], vec![i]),
        (vec![i, i, i, i], vec![i]),
        (vec![i, i, i], vec![l]),
        (vec![i, i, i], vec![]),
        (vec![i, i, i, l], vec![]),
        (vec![i, i], vec![]),
        (vec![i], vec![i]),
        (vec![i, i], vec![l]),
        (vec![i], vec![]),
    ] {
        types.ty().function(params, result);
    }
    for (params, result) in &image.extra_types {
        let result: &[ValType] = match result {
            Some(kind) => std::slice::from_ref(kind),
            None => &[],
        };
        types
            .ty()
            .function(params.iter().copied(), result.iter().copied());
    }

    let mut imports = ImportSection::new();
    imports.import(
        "shamir_host",
        "raise_error",
        EntityType::Function(TYPE_VOID_THREE_I32),
    );
    for (name, type_index) in &image.db_imports {
        imports.import("shamir_host", name, EntityType::Function(*type_index));
    }

    let mut functions = FunctionSection::new();
    for (type_index, _) in &image.helper_functions {
        functions.function(*type_index);
    }
    functions.function(TYPE_VOID_THREE_I32);
    functions.function(TYPE_ALLOC);
    functions.function(TYPE_CALL);
    for (type_index, _) in &image.user_functions {
        functions.function(*type_index);
    }
    for (type_index, _) in &image.eq_functions {
        functions.function(*type_index);
    }
    for (type_index, _) in &image.projection_functions {
        functions.function(*type_index);
    }
    for (type_index, _) in &image.db_projection_functions {
        functions.function(*type_index);
    }
    for (type_index, _) in &image.db_call_functions {
        functions.function(*type_index);
    }

    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: image.layout.minimum_pages,
        maximum: Some(image.layout.maximum_pages),
        memory64: false,
        shared: false,
        page_size_log2: None,
    });

    let mut globals = GlobalSection::new();
    // two mutable i32 globals per design §5: heap_next (index 0) and
    // call_state (index 1, the wrapper re-entry guard)
    let heap_const = ConstExpr::i32_const(i32::try_from(image.heap_base).map_err(|_| {
        RuntimeError::RuntimeTooLarge {
            limit: i32::MAX as u64,
            observed: image.heap_base,
        }
    })?);
    let global_type = GlobalType {
        val_type: ValType::I32,
        mutable: true,
        shared: false,
    };
    globals.global(global_type, &heap_const);
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(0),
    );
    let mut exports = ExportSection::new();
    exports.export(EXPORT_MEMORY, ExportKind::Memory, 0);
    exports.export(EXPORT_ALLOC, ExportKind::Func, image.alloc_index);
    exports.export(EXPORT_CALL, ExportKind::Func, image.call_index);

    let mut code = CodeSection::new();
    for (_, function) in &image.helper_functions {
        code.function(function);
    }
    code.function(&image.error_raiser);
    code.function(&image.allocator);
    code.function(&image.wrapper);
    for (_, function) in &image.user_functions {
        code.function(function);
    }
    for (_, function) in &image.eq_functions {
        code.function(function);
    }
    for (_, function) in &image.projection_functions {
        code.function(function);
    }
    for (_, function) in &image.db_projection_functions {
        code.function(function);
    }
    for (_, function) in &image.db_call_functions {
        code.function(function);
    }

    let mut data = DataSection::new();
    let static_const = ConstExpr::i32_const(i32::try_from(STATIC_START).map_err(|_| {
        RuntimeError::RuntimeTooLarge {
            limit: i32::MAX as u64,
            observed: STATIC_START,
        }
    })?);
    data.active(0, &static_const, image.static_bytes.to_vec());

    let mut module = Module::new();
    module.section(&CustomSection {
        name: ABI_SECTION.into(),
        data: SUPPORTED_ABI_VERSION.to_le_bytes().to_vec().into(),
    });
    module.section(&CustomSection {
        name: ORIGIN_SECTION.into(),
        data: b"shamir-lang".to_vec().into(),
    });
    module.section(&types);
    module.section(&imports);
    module.section(&functions);
    module.section(&memories);
    module.section(&globals);
    module.section(&exports);
    module.section(&code);
    module.section(&data);

    let bytes = module.finish();
    let mut code_bytes = Vec::new();
    code.encode(&mut code_bytes);
    let code_len = u64::try_from(code_bytes.len()).unwrap_or(u64::MAX);
    Ok((bytes, code_len))
}

/// Deduplicates one signature by linear scan over the extra types
/// (first-use order, deterministic); the runtime occupies types 0..=9.
pub(crate) fn intern_type(
    extra_types: &mut Vec<(Vec<ValType>, Option<ValType>)>,
    params: Vec<ValType>,
    result: Option<ValType>,
) -> Result<u32, EmitError> {
    if let Some(position) = extra_types
        .iter()
        .position(|(existing, existing_result)| *existing == params && *existing_result == result)
    {
        let index =
            u32::try_from(position).map_err(|_| EmitError::Internal("type index overflow"))?;
        return 10u32
            .checked_add(index)
            .ok_or(EmitError::Internal("type index overflow"));
    }
    let position =
        u32::try_from(extra_types.len()).map_err(|_| EmitError::Internal("type index overflow"))?;
    extra_types.push((params, result));
    let observed = 10u64
        .checked_add(u64::from(position) + 1)
        .ok_or(EmitError::Internal("type count overflow"))?;
    if observed > MAX_TYPES {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit: MAX_TYPES,
            observed,
        });
    }
    10u32
        .checked_add(position)
        .ok_or(EmitError::Internal("type index overflow"))
}

pub(crate) fn val_type(kind: ValueKind) -> ValType {
    match kind {
        ValueKind::I32 => ValType::I32,
        ValueKind::I64 => ValType::I64,
        ValueKind::F64 => ValType::F64,
    }
}
