//! Scalar IR-function emission: one `IrFunction` body becomes one wasm
//! `Function` under the operand-id contract. Every value-producing
//! instruction owns one dense wasm local (declared after the fixed scratch
//! tail, grouped by result kind in instruction order); operand reads are
//! `local.get`s and results are `local.set`s, so the wasm stack is only used
//! inside single instructions and control structures. Control flow is
//! compiled by `control::drive` through an explicit worklist (no recursion),
//! and this module emits the closing final `End` of the function body.

use wasm_encoder::{Function, Instruction, MemArg, ValType};

use super::module::{EmissionPlan, EmitContext, EmitError};
use crate::back::ir::inst_result_kind;
use crate::back::ir::{BlockId, IrFunction, ValueKind};

use control::EmitEnv;

mod arith;
mod control;
mod insts;

/// Maximum wasm nesting depth (loops + ifs) allowed per function body.
const MAX_WASM_DEPTH: usize = 256;

/// Fixed scratch locals appended after the declared locals, before the dense
/// instruction locals. Relative to the slot base (`params + locals`): s0/s1
/// are i32 temps (+0/+1), s2..s4 i64 temps (+2..+4), s5 an f64 temp (+5).
/// Instruction value locals start at `slot_base + 6`.
const SCRATCH_DECL: [(u32, ValType); 4] = [
    (1, ValType::I32),
    (1, ValType::I32),
    (3, ValType::I64),
    (1, ValType::F64),
];

/// Fixed raise messages appended to the static image in this order by
/// `emit_compiled_module`; their offsets land in `EmitContext`.
pub(crate) const RAISE_MESSAGES: [&[u8]; 5] = [b"ovf", b"div0", b"range", b"bound", b"unwrap"];

/// Worklist entry.
pub(super) enum Task {
    /// Emit a block chain starting at the block (skipped when already
    /// emitted).
    Block(BlockId),
    /// Emit one `if` arm inside the open `if`.
    Arm(BlockId),
    /// Switch the open `if` to its else arm.
    ElseMark,
    /// Close the `if` and pop its label frame.
    IfEnd,
    /// Close the loop's `loop` + outer `block` and pop the label frame.
    LoopEnds,
}

/// One label-stack frame: loop frames name their header (and, once the exit
/// conditional was seen, the exit/join block); if frames record the arm
/// fall-through join.
pub(super) struct LabelFrame {
    pub(super) loop_header: Option<BlockId>,
    pub(super) join: Option<BlockId>,
    pub(super) fallthrough: Option<BlockId>,
}

pub(super) fn val_type(kind: ValueKind) -> ValType {
    match kind {
        ValueKind::I32 => ValType::I32,
        ValueKind::I64 => ValType::I64,
        ValueKind::F64 => ValType::F64,
    }
}

fn run_length_locals(locals: &[ValueKind]) -> Vec<(u32, ValType)> {
    let mut counts: Vec<(u32, ValType)> = Vec::new();
    for kind in locals {
        let kind = val_type(*kind);
        match counts.last_mut() {
            Some((count, last)) if *last == kind => *count += 1,
            _ => counts.push((1, kind)),
        }
    }
    counts
}

pub(super) fn mem32(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 2,
        memory_index: 0,
    }
}

pub(super) fn mem64(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 3,
        memory_index: 0,
    }
}

pub(super) fn slot_base(func: &IrFunction) -> u32 {
    func.params.len().saturating_add(func.locals.len()) as u32
}

/// Passes a static-image offset through unchanged: builder offsets are
/// already ABSOLUTE linear addresses (STATIC_BASE baked in by the builder).
/// Only the i32 bound check remains.
pub(super) fn static_addr(offset: u32) -> Result<u32, EmitError> {
    if offset <= i32::MAX as u32 {
        Ok(offset)
    } else {
        Err(EmitError::Internal("static address exceeds i32"))
    }
}

/// Kind-2 raise against embedded static message bytes, then `unreachable`
/// (the error bridge never returns).
pub(super) fn raise_static(
    plan: &EmissionPlan,
    function: &mut Function,
    kind: i32,
    addr: u32,
    message: &'static [u8],
) -> Result<(), EmitError> {
    let len = u32::try_from(message.len()).map_err(|_| EmitError::Internal("message too long"))?;
    function.instruction(&Instruction::I32Const(kind));
    function.instruction(&Instruction::I32Const(addr as i32));
    function.instruction(&Instruction::I32Const(len as i32));
    function.instruction(&Instruction::Call(plan.raise_index));
    function.instruction(&Instruction::Unreachable);
    Ok(())
}

/// Per-instruction value-local map plus the grouped local declarations.
pub(super) type LocalPlan = (Vec<Option<u32>>, Vec<(u32, ValType)>);

/// Dense value-local numbering: one local per value-producing instruction
/// (`inst_result_kind(...).is_some()`), starting after the six scratch
/// locals, grouped by result kind in instruction order; `None` for non-value
/// instructions. Returns the id map and the run-length declarations.
fn value_local_map(
    func: &IrFunction,
    functions: &[IrFunction],
    base: u32,
) -> Result<LocalPlan, EmitError> {
    let mut map = Vec::with_capacity(func.insts.len());
    let mut decls: Vec<(u32, ValType)> = Vec::new();
    let mut dense: u32 = 0;
    for inst in &func.insts {
        match inst_result_kind(&inst.kind, func, functions) {
            Some(kind) => {
                let local = base
                    .checked_add(6)
                    .and_then(|scratch_end| scratch_end.checked_add(dense))
                    .ok_or(EmitError::Internal("local index overflow"))?;
                dense = dense
                    .checked_add(1)
                    .ok_or(EmitError::Internal("local count overflow"))?;
                map.push(Some(local));
                let kind = val_type(kind);
                match decls.last_mut() {
                    Some((count, last)) if *last == kind => *count += 1,
                    _ => decls.push((1, kind)),
                }
            }
            None => map.push(None),
        }
    }
    Ok((map, decls))
}

/// Emits one user function body. `fn_index` is the defined-function index of
/// `func` (`plan.users_base + ir_index`).
pub(crate) fn emit_user_function(
    plan: &EmissionPlan,
    ctx: &EmitContext,
    _fn_index: usize,
    func: &IrFunction,
    functions: &[IrFunction],
) -> Result<Function, EmitError> {
    let base = slot_base(func);
    let (inst_local, inst_decls) = value_local_map(func, functions, base)?;
    let mut local_decls = run_length_locals(&func.locals);
    local_decls.extend_from_slice(&SCRATCH_DECL);
    local_decls.extend(inst_decls);
    let mut function = Function::new(local_decls);
    let env = EmitEnv {
        plan,
        ctx,
        func,
        base,
        inst_local,
    };
    control::drive(&env, &mut function)?;
    // The function body's final `End`.
    function.instruction(&Instruction::End);
    Ok(function)
}

pub(super) fn check_depth(labels: &[LabelFrame]) -> Result<(), EmitError> {
    if labels.len() > MAX_WASM_DEPTH {
        return Err(EmitError::LimitExceeded {
            code: "B001",
            limit: MAX_WASM_DEPTH as u64,
            observed: labels.len() as u64,
        });
    }
    Ok(())
}
