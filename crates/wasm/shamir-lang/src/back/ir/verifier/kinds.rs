use crate::back::ir::builder::STATIC_BASE;
use crate::back::ir::ids::InstId;
use crate::back::ir::module::Inst;
use crate::back::ir::module::IrFunction;
use crate::back::ir::module::IrModule;
use crate::back::ir::ops::inst_operands;
use crate::back::ir::ops::inst_result_kind;
use crate::back::ir::ops::local_slot_kind;
use crate::back::ir::ops::CastOp;
use crate::back::ir::ops::InstKind;
use crate::back::ir::ops::Terminator;
use crate::back::ir::ops::ValueKind;

use super::kind_error;
use super::out_of_range;
use super::tile_error;
use super::VerifierError;

// Rules 1, 2, 3 for one block: id ranges, operand locality, kind consistency.
pub(crate) fn verify_block(
    function_index: u32,
    function: &IrFunction,
    block_index: usize,
    module: &IrModule,
) -> Result<(), VerifierError> {
    let block = match function.blocks.get(block_index) {
        Some(block) => block,
        None => {
            return Err(tile_error(
                function_index,
                block_index,
                String::from("missing block"),
            ))
        }
    };
    let block_start = block.start.0;
    for offset in 0..block.inst_count {
        let inst_id = block_start.checked_add(offset).ok_or_else(|| {
            tile_error(function_index, block_index, String::from("range overflow"))
        })?;
        let inst = inst_at(function, inst_id, function_index, block_index)?;
        verify_inst(function_index, function, block_index, inst_id, inst, module)?;
    }
    verify_terminator(function_index, function, block_index, module)?;
    Ok(())
}

fn inst_at(
    function: &IrFunction,
    inst_id: u32,
    function_index: u32,
    block_index: usize,
) -> Result<&Inst, VerifierError> {
    function.insts.get(inst_id as usize).ok_or_else(|| {
        out_of_range(
            function_index,
            block_index,
            inst_id,
            format!("instruction id {}", inst_id),
        )
    })
}

// Rules 1, 2, 3 for one instruction.
fn verify_inst(
    function_index: u32,
    function: &IrFunction,
    block_index: usize,
    inst_id: u32,
    inst: &Inst,
    module: &IrModule,
) -> Result<(), VerifierError> {
    let mut operands = Vec::new();
    inst_operands(&inst.kind, &mut operands);
    // Rule 1: every operand id must be in range.
    for operand in &operands {
        if operand.0 >= function.insts.len() as u32 {
            return Err(out_of_range(
                function_index,
                block_index,
                inst_id,
                format!("operand id {}", operand.0),
            ));
        }
    }
    // Operator-specific id ranges.
    match &inst.kind {
        InstKind::Call { callee, .. } if *callee >= module.functions.len() as u32 => {
            return Err(out_of_range(
                function_index,
                block_index,
                inst_id,
                format!("callee index {}", callee),
            ));
        }
        InstKind::Call { .. } => {}
        InstKind::RecordCell { descriptor, .. }
            // Descriptors are ABSOLUTE (STATIC_BASE + image offset).
            if *descriptor as u64
                >= STATIC_BASE.saturating_add(module.static_bytes.len() as u64) =>
        {
            return Err(out_of_range(
                function_index,
                block_index,
                inst_id,
                format!("descriptor offset {}", descriptor),
            ));
        }
        InstKind::RecordCell { .. } => {}
        InstKind::StructEq { shape, .. } if *shape >= module.eq_shapes.len() as u32 => {
            return Err(out_of_range(
                function_index,
                block_index,
                inst_id,
                format!("shape index {}", shape),
            ));
        }
        InstKind::StructEq { .. } => {}
        InstKind::LocalGet(slot) | InstKind::LocalSet(slot) => {
            let slot_limit = slot_limit(function)?;
            if *slot >= slot_limit {
                return Err(out_of_range(
                    function_index,
                    block_index,
                    inst_id,
                    format!("slot {} of {}", slot, slot_limit),
                ));
            }
        }
        _ => {}
    }
    // Rule 2: operands must belong to the same block, and rule 2b: they must
    // be defined STRICTLY EARLIER in that block.
    for operand in &operands {
        if function.block_of(operand.0) != Some(block_index) {
            return Err(VerifierError::OperandNotInBlock {
                function: function_index,
                block: block_index as u32,
                inst: inst_id,
                detail: format!("operand id {} used outside its block", operand.0),
            });
        }
        if operand.0 >= inst_id {
            return Err(VerifierError::OperandNotInBlock {
                function: function_index,
                block: block_index as u32,
                inst: inst_id,
                detail: format!("operand id {} defined later", operand.0),
            });
        }
    }
    // Rule 3: kind consistency.
    check_inst_kinds(function_index, function, block_index, inst_id, inst, module)?;
    Ok(())
}

pub(crate) fn slot_limit(function: &IrFunction) -> Result<u32, VerifierError> {
    let params = u32::try_from(function.params.len()).map_err(|_| VerifierError::Internal {
        function: 0,
        detail: String::from("parameter count overflow"),
    })?;
    let locals = u32::try_from(function.locals.len()).map_err(|_| VerifierError::Internal {
        function: 0,
        detail: String::from("local count overflow"),
    })?;
    params.checked_add(locals).ok_or(VerifierError::Internal {
        function: 0,
        detail: String::from("slot space overflow"),
    })
}

// Rule 3 for one instruction (id ranges already checked).
fn check_inst_kinds(
    function_index: u32,
    function: &IrFunction,
    block_index: usize,
    inst_id: u32,
    inst: &Inst,
    module: &IrModule,
) -> Result<(), VerifierError> {
    let kind = |operand: InstId| -> Result<ValueKind, VerifierError> {
        let operand_inst = function.insts.get(operand.index()).ok_or_else(|| {
            out_of_range(
                function_index,
                block_index,
                inst_id,
                format!("operand id {}", operand.0),
            )
        })?;
        inst_result_kind(&operand_inst.kind, function, &module.functions).ok_or_else(|| {
            kind_error(
                function_index,
                block_index,
                inst_id,
                String::from("operand has no value"),
            )
        })
    };
    let want = |observed: Result<ValueKind, VerifierError>,
                expected: ValueKind,
                detail: String|
     -> Result<(), VerifierError> {
        match observed {
            Ok(kind) if kind == expected => Ok(()),
            Ok(kind) => Err(kind_error(
                function_index,
                block_index,
                inst_id,
                format!("{}: expected {:?}, got {:?}", detail, expected, kind),
            )),
            Err(error) => Err(error),
        }
    };
    match &inst.kind {
        InstKind::CheckedArith {
            width, left, right, ..
        } => {
            if *width == ValueKind::F64 {
                return Err(kind_error(
                    function_index,
                    block_index,
                    inst_id,
                    String::from("CheckedArith width must be I32 or I64"),
                ));
            }
            let left_kind = kind(*left)?;
            let right_kind = kind(*right)?;
            want(Ok(left_kind), *width, String::from("left operand"))?;
            want(Ok(right_kind), *width, String::from("right operand"))?;
        }
        InstKind::FloatArith { left, right, .. } => {
            want(kind(*left), ValueKind::F64, String::from("left operand"))?;
            want(kind(*right), ValueKind::F64, String::from("right operand"))?;
        }
        InstKind::FloatNeg(value) => want(kind(*value), ValueKind::F64, String::from("operand"))?,
        InstKind::Compare {
            width, left, right, ..
        } => {
            want(kind(*left), *width, String::from("left operand"))?;
            want(kind(*right), *width, String::from("right operand"))?;
        }
        InstKind::Cast { op, value } => {
            let source = match op {
                CastOp::ToI64FromI32 | CastOp::ToF64FromI32 => ValueKind::I32,
                CastOp::ToF64FromI64 | CastOp::ToI32FromI64 => ValueKind::I64,
                CastOp::ToI32FromF64 | CastOp::ToI64FromF64 => ValueKind::F64,
            };
            want(kind(*value), source, String::from("cast operand"))?;
        }
        InstKind::BoolNot(value) => want(kind(*value), ValueKind::I32, String::from("operand"))?,
        InstKind::SExtend64(value) => want(kind(*value), ValueKind::I32, String::from("operand"))?,
        InstKind::I32WrapI64(value) => want(kind(*value), ValueKind::I64, String::from("operand"))?,
        InstKind::F64FromBits(value) => {
            want(kind(*value), ValueKind::I64, String::from("operand"))?
        }
        InstKind::CellAllocI64(value) => {
            want(kind(*value), ValueKind::I64, String::from("operand"))?
        }
        InstKind::CellAllocBool(value) => {
            want(kind(*value), ValueKind::I32, String::from("operand"))?
        }
        InstKind::CellAllocF64(value) => {
            want(kind(*value), ValueKind::F64, String::from("operand"))?
        }
        InstKind::RecordCell { region, .. } => want(
            kind(*region),
            ValueKind::I32,
            String::from("region operand"),
        )?,
        InstKind::CellsAlloc(count) => {
            want(kind(*count), ValueKind::I32, String::from("count operand"))?
        }
        InstKind::CellLoadPayload(cell)
        | InstKind::CellIsNull(cell)
        | InstKind::CellUnwrap(cell)
        | InstKind::CellLen(cell) => {
            want(kind(*cell), ValueKind::I32, String::from("cell operand"))?
        }
        InstKind::CellStoreAux { cell, value } => {
            want(kind(*cell), ValueKind::I32, String::from("cell operand"))?;
            want(kind(*value), ValueKind::I32, String::from("aux value"))?;
        }
        InstKind::CellStorePayload { cell, value } => {
            want(kind(*cell), ValueKind::I32, String::from("cell operand"))?;
            want(kind(*value), ValueKind::I64, String::from("payload value"))?;
        }
        InstKind::CellCopyTo { dest, source } => {
            want(kind(*dest), ValueKind::I32, String::from("dest operand"))?;
            want(
                kind(*source),
                ValueKind::I32,
                String::from("source operand"),
            )?;
        }
        InstKind::CellElemPtr { base, index } | InstKind::CellSlot { base, index } => {
            want(kind(*base), ValueKind::I32, String::from("base operand"))?;
            want(kind(*index), ValueKind::I32, String::from("index operand"))?;
        }
        InstKind::BytesCompare { left, right } => {
            want(kind(*left), ValueKind::I32, String::from("left operand"))?;
            want(kind(*right), ValueKind::I32, String::from("right operand"))?;
        }
        InstKind::StructEq { left, right, .. } => {
            want(kind(*left), ValueKind::I32, String::from("left operand"))?;
            want(kind(*right), ValueKind::I32, String::from("right operand"))?;
        }
        InstKind::Call { callee, args } => {
            let callee_function = match module.functions.get(*callee as usize) {
                Some(callee_function) => callee_function,
                None => {
                    return Err(out_of_range(
                        function_index,
                        block_index,
                        inst_id,
                        format!("callee index {}", callee),
                    ))
                }
            };
            if args.len() != callee_function.params.len() {
                return Err(kind_error(
                    function_index,
                    block_index,
                    inst_id,
                    format!(
                        "callee expects {} args, got {}",
                        callee_function.params.len(),
                        args.len()
                    ),
                ));
            }
            for (position, arg) in args.iter().enumerate() {
                let expected = callee_function.params.get(position).ok_or_else(|| {
                    out_of_range(
                        function_index,
                        block_index,
                        inst_id,
                        String::from("arg position"),
                    )
                })?;
                want(kind(*arg), *expected, format!("arg {}", position))?;
            }
        }
        InstKind::LocalSet(slot) => {
            let slot_kind = local_slot_kind(*slot, function).ok_or_else(|| {
                out_of_range(
                    function_index,
                    block_index,
                    inst_id,
                    format!("slot {}", slot),
                )
            })?;
            // LocalSet pops its value from the preceding instruction.
            if inst_id
                <= function
                    .blocks
                    .get(block_index)
                    .map_or(u32::MAX, |b| b.start.0)
            {
                return Err(kind_error(
                    function_index,
                    block_index,
                    inst_id,
                    String::from("LocalSet without a preceding value instruction"),
                ));
            }
            let value = InstId(inst_id - 1);
            want(kind(value), slot_kind, format!("set of slot {}", slot))?;
        }
        InstKind::LocalGet(slot) => {
            if local_slot_kind(*slot, function).is_none() {
                return Err(out_of_range(
                    function_index,
                    block_index,
                    inst_id,
                    format!("slot {}", slot),
                ));
            }
        }
        InstKind::Drop(_) => {}
        InstKind::ConstI32(_)
        | InstKind::ConstI64(_)
        | InstKind::ConstF64(_)
        | InstKind::StaticCell(_)
        | InstKind::CellAlloc { .. } => {}
    }
    Ok(())
}

// Rules 1, 2, 3 for the terminator of one block.
pub(crate) fn verify_terminator(
    function_index: u32,
    function: &IrFunction,
    block_index: usize,
    module: &IrModule,
) -> Result<(), VerifierError> {
    let block = function
        .blocks
        .get(block_index)
        .ok_or_else(|| tile_error(function_index, block_index, String::from("missing block")))?;
    let check_operand =
        |operand: InstId, detail: &'static str| -> Result<ValueKind, VerifierError> {
            if operand.0 >= function.insts.len() as u32 {
                return Err(out_of_range(
                    function_index,
                    block_index,
                    block.start.0,
                    format!("{} id {}", detail, operand.0),
                ));
            }
            if function.block_of(operand.0) != Some(block_index) {
                return Err(VerifierError::OperandNotInBlock {
                    function: function_index,
                    block: block_index as u32,
                    inst: operand.0,
                    detail: format!("{} id {} used outside its block", detail, operand.0),
                });
            }
            let inst = function.insts.get(operand.index()).ok_or_else(|| {
                out_of_range(
                    function_index,
                    block_index,
                    operand.0,
                    format!("{} id {}", detail, operand.0),
                )
            })?;
            inst_result_kind(&inst.kind, function, &module.functions).ok_or_else(|| {
                kind_error(
                    function_index,
                    block_index,
                    operand.0,
                    String::from("terminator operand has no value"),
                )
            })
        };
    match &block.terminator {
        Terminator::Return(Some(value)) => {
            let observed = check_operand(*value, "return value")?;
            let expected = function.result.ok_or_else(|| {
                kind_error(
                    function_index,
                    block_index,
                    value.0,
                    String::from("return value in a void function"),
                )
            })?;
            if observed != expected {
                return Err(kind_error(
                    function_index,
                    block_index,
                    value.0,
                    format!(
                        "return kind {:?} but function result is {:?}",
                        observed, expected
                    ),
                ));
            }
        }
        Terminator::Return(None) => {
            if function.result.is_some() {
                return Err(kind_error(
                    function_index,
                    block_index,
                    block.start.0,
                    String::from("bare return in a value-returning function"),
                ));
            }
        }
        Terminator::TrapCell(message) => {
            let observed = check_operand(*message, "trap cell")?;
            if observed != ValueKind::I32 {
                return Err(kind_error(
                    function_index,
                    block_index,
                    message.0,
                    format!("trap cell must be I32, got {:?}", observed),
                ));
            }
        }
        Terminator::Branch(_) => {}
        Terminator::BranchIf { cond, .. } => {
            let observed = check_operand(*cond, "branch cond")?;
            if observed != ValueKind::I32 {
                return Err(kind_error(
                    function_index,
                    block_index,
                    cond.0,
                    format!("branch cond must be I32, got {:?}", observed),
                ));
            }
        }
        Terminator::Unreachable => {}
    }
    Ok(())
}
