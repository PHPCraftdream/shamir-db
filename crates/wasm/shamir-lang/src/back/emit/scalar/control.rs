//! Control-flow compilation of one scalar IR function under the operand-id
//! contract: every value-producing instruction owns one wasm local, operands
//! are always read with `local.get`, and `LocalSet` copies the immediately
//! preceding instruction's local into its slot (the IR contract).
//!
//! Blocks are compiled through an explicit LIFO worklist. A `Branch` falls
//! through (emits nothing) exactly when its target is the next not-yet-emitted
//! block reachable by continuing the current inline chain (this covers both
//! the if-join chaining and the for-loop `body -> incr` chaining); every other
//! target must resolve to an enclosing loop label (`loop_header` or the loop
//! `join`/exit). Loop blocks emit the wasm `block { loop { ... } }` shape.

use wasm_encoder::{BlockType, Function, Instruction};

use crate::back::emit::module::{EmissionPlan, EmitContext, EmitError};
use crate::back::ir::{BlockId, BlockKind, InstId, IrFunction, Terminator};

use super::{check_depth, mem32, mem64, LabelFrame, Task};

/// Shared read-only emission context threaded through the scalar body
/// emitters; the mutable wasm `Function`, label stack and worklist stay
/// separate `&mut` arguments.
pub(super) struct EmitEnv<'a> {
    pub(super) plan: &'a EmissionPlan,
    pub(super) ctx: &'a EmitContext,
    pub(super) func: &'a IrFunction,
    /// First scratch local (`params + ir locals`).
    pub(super) base: u32,
    /// `InstId -> Option<value local>`; `None` for non-value instructions.
    pub(super) inst_local: Vec<Option<u32>>,
}

impl EmitEnv<'_> {
    /// The value local of one operand instruction.
    pub(super) fn local_of(&self, id: InstId) -> Result<u32, EmitError> {
        self.inst_local
            .get(id.index())
            .copied()
            .flatten()
            .ok_or(EmitError::Internal("operand without a result local"))
    }

    /// The wasm local holding one instruction's own result.
    pub(super) fn result_local(&self, position: usize) -> Option<u32> {
        self.inst_local.get(position).copied().flatten()
    }

    /// i32 scratch locals (s0 at `base`, s1 at `base + 1`).
    pub(super) fn s0(&self) -> u32 {
        self.base
    }

    pub(super) fn s1(&self) -> u32 {
        self.base.saturating_add(1)
    }
}

/// Drives the whole function body: entry block through the worklist, then the
/// caller emits the final `Instruction::End` closing the function body.
pub(super) fn drive(env: &EmitEnv, function: &mut Function) -> Result<(), EmitError> {
    let mut labels: Vec<LabelFrame> = Vec::new();
    let mut tasks: Vec<Task> = Vec::new();
    let mut seen = vec![false; env.func.blocks.len()];
    if seen.is_empty() {
        return Err(EmitError::Internal("user function without blocks"));
    }
    tasks.push(Task::Block(BlockId(0)));
    while let Some(task) = tasks.pop() {
        match task {
            Task::Block(block) => {
                let index = block.0 as usize;
                if index >= seen.len() {
                    return Err(EmitError::Internal("block index out of range"));
                }
                // A join already emitted inside an arm (no-else lowering)
                // must not be emitted twice.
                if !seen[index] {
                    emit_chain(env, function, &mut labels, &mut tasks, &mut seen, block)?;
                }
            }
            Task::Arm(block) => {
                emit_arm(env, function, &mut labels, &mut tasks, &mut seen, block)?;
            }
            Task::ElseMark => {
                function.instruction(&Instruction::Else);
            }
            Task::IfEnd => {
                let frame = labels.pop().ok_or(EmitError::Internal("label underflow"))?;
                if frame.loop_header.is_some() {
                    return Err(EmitError::Internal("if end over a loop frame"));
                }
                function.instruction(&Instruction::End);
                // An arm fell into the join: it is emitted right after the
                // `if` closes (already-emitted joins are skipped by `seen`).
                if let Some(join) = frame.fallthrough {
                    tasks.push(Task::Block(join));
                }
            }
            Task::LoopEnds => {
                let frame = labels.pop().ok_or(EmitError::Internal("label underflow"))?;
                if frame.loop_header.is_none() {
                    return Err(EmitError::Internal("loop end over an if frame"));
                }
                function.instruction(&Instruction::End);
                function.instruction(&Instruction::End);
            }
        }
    }
    if !labels.is_empty() {
        return Err(EmitError::Internal("unbalanced label frames"));
    }
    Ok(())
}

/// Emits one block's instructions in arena order.
fn emit_block_insts(
    env: &EmitEnv,
    function: &mut Function,
    block: BlockId,
) -> Result<(), EmitError> {
    let (_kind, start, count, _terminator) = block_parts(env.func, block)?;
    for offset in 0..count as usize {
        let position = (start
            .checked_add(offset as u32)
            .ok_or(EmitError::Internal("instruction index overflow"))?)
            as usize;
        let inst = env
            .func
            .insts
            .get(position)
            .ok_or(EmitError::Internal("instruction out of range"))?;
        super::insts::emit_inst(env, function, position, inst)?;
    }
    Ok(())
}

fn block_parts(
    func: &IrFunction,
    block: BlockId,
) -> Result<(BlockKind, u32, u32, &Terminator), EmitError> {
    let data = func
        .blocks
        .get(block.0 as usize)
        .ok_or(EmitError::Internal("block index out of range"))?;
    Ok((data.kind, data.start.0, data.inst_count, &data.terminator))
}

/// Marks `target` as the chain continuation; rejected when out of range or
/// already emitted (a real back edge always resolves to a loop label first).
fn fresh_target(seen: &[bool], target: BlockId) -> Result<BlockId, EmitError> {
    let index = target.0 as usize;
    if index >= seen.len() {
        return Err(EmitError::Internal("branch target out of range"));
    }
    if seen[index] {
        return Err(EmitError::Internal("branch to an emitted non-loop block"));
    }
    Ok(target)
}

/// Emits a chain of plain blocks inline, ending on a terminator that leaves
/// the chain (return/trap/unreachable, a branch, or an `if` whose arms are
/// deferred through the worklist).
fn emit_chain(
    env: &EmitEnv,
    function: &mut Function,
    labels: &mut Vec<LabelFrame>,
    tasks: &mut Vec<Task>,
    seen: &mut [bool],
    start: BlockId,
) -> Result<(), EmitError> {
    let mut current = Some(start);
    while let Some(block) = current {
        let index = block.0 as usize;
        if index >= seen.len() {
            return Err(EmitError::Internal("block index out of range"));
        }
        if seen[index] {
            return Err(EmitError::Internal("chain re-entered an emitted block"));
        }
        seen[index] = true;
        let (kind, _start, _count, terminator) = block_parts(env.func, block)?;
        if kind == BlockKind::Loop {
            labels.push(LabelFrame {
                loop_header: Some(block),
                join: None,
                fallthrough: None,
            });
            check_depth(labels)?;
            function.instruction(&Instruction::Block(BlockType::Empty));
            function.instruction(&Instruction::Loop(BlockType::Empty));
            tasks.push(Task::LoopEnds);
        }
        emit_block_insts(env, function, block)?;
        match terminator {
            Terminator::Return(Some(value)) => {
                function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
                function.instruction(&Instruction::Return);
                return Ok(());
            }
            Terminator::Return(None) => {
                function.instruction(&Instruction::Return);
                return Ok(());
            }
            Terminator::TrapCell(message) => {
                emit_trap_cell(env, function, *message)?;
                return Ok(());
            }
            Terminator::Unreachable => {
                function.instruction(&Instruction::Unreachable);
                return Ok(());
            }
            Terminator::Branch(target) => {
                if let Some(depth) = loop_target_depth(labels, *target) {
                    function.instruction(&Instruction::Br(depth));
                    return Ok(());
                }
                current = Some(fresh_target(seen, *target)?);
            }
            Terminator::BranchIf { cond, then, else_ } => {
                // A loop header's own conditional: the header's frame is the
                // innermost loop frame and was pushed in this chain step, so
                // `br_if 1` names the loop's outer `block` label.
                if labels.iter().any(|f| f.loop_header == Some(block)) {
                    open_loop_exit(env, function, labels, tasks, *cond, *then, *else_)?;
                    // The other arm continues inline inside the loop.
                    let exit = if then.0 > else_.0 { *then } else { *else_ };
                    current = Some(if exit == *then { *else_ } else { *then });
                } else {
                    open_if(env, function, labels, tasks, *cond, *then, *else_)?;
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

/// Loop-header conditional: the exit arm carries the LARGER block index
/// (lowering creates the exit block after the body/increment blocks, so
/// `while` = `BranchIf(body, exit)` and `for` = `BranchIf(exit, body)` both
/// resolve by the max-index rule). Emits `cond; br_if(<outer block label>)`,
/// records the exit as the loop frame's join (so `break` resolves), defers
/// the exit block behind `LoopEnds`, and lets the caller continue inline with
/// the body arm.
fn open_loop_exit(
    env: &EmitEnv,
    function: &mut Function,
    labels: &mut [LabelFrame],
    tasks: &mut Vec<Task>,
    cond: InstId,
    then: BlockId,
    else_: BlockId,
) -> Result<(), EmitError> {
    let exit = if then.0 > else_.0 { then } else { else_ };
    let frame = labels
        .last_mut()
        .ok_or(EmitError::Internal("missing loop frame"))?;
    frame.join = Some(exit);
    function.instruction(&Instruction::LocalGet(env.local_of(cond)?));
    function.instruction(&Instruction::BrIf(1)); // 1 = the loop's outer `block`
    tasks.push(Task::Block(exit));
    tasks.push(Task::LoopEnds);
    Ok(())
}

/// Opens a wasm `if (void)` over the condition local and defers both arms and
/// the closing `end` through the worklist; usable at chain level and at the
/// end of an arm (nested ifs).
fn open_if(
    env: &EmitEnv,
    function: &mut Function,
    labels: &mut Vec<LabelFrame>,
    tasks: &mut Vec<Task>,
    cond: InstId,
    then: BlockId,
    else_: BlockId,
) -> Result<(), EmitError> {
    labels.push(LabelFrame {
        loop_header: None,
        join: None,
        fallthrough: None,
    });
    check_depth(labels)?;
    function.instruction(&Instruction::LocalGet(env.local_of(cond)?));
    function.instruction(&Instruction::If(BlockType::Empty));
    tasks.push(Task::IfEnd);
    tasks.push(Task::Arm(else_));
    tasks.push(Task::ElseMark);
    tasks.push(Task::Arm(then));
    Ok(())
}

/// Emits one `if` arm. A `Branch` resolves to an enclosing loop label or
/// records the arm's fall-through join (both arms must agree); a nested
/// `BranchIf` opens the inner `if` and defers its arms.
fn emit_arm(
    env: &EmitEnv,
    function: &mut Function,
    labels: &mut Vec<LabelFrame>,
    tasks: &mut Vec<Task>,
    seen: &mut [bool],
    block: BlockId,
) -> Result<(), EmitError> {
    let index = block.0 as usize;
    if index >= seen.len() {
        return Err(EmitError::Internal("arm block out of range"));
    }
    if seen[index] {
        // e.g. the no-else lowering branches straight to the join block,
        // which is emitted inside this arm before `IfEnd` re-schedules it.
        return Ok(());
    }
    seen[index] = true;
    let (_kind, _start, _count, terminator) = block_parts(env.func, block)?;
    emit_block_insts(env, function, block)?;
    match terminator {
        Terminator::Return(Some(value)) => {
            function.instruction(&Instruction::LocalGet(env.local_of(*value)?));
            function.instruction(&Instruction::Return);
        }
        Terminator::Return(None) => {
            function.instruction(&Instruction::Return);
        }
        Terminator::TrapCell(message) => {
            emit_trap_cell(env, function, *message)?;
        }
        Terminator::Unreachable => {
            function.instruction(&Instruction::Unreachable);
        }
        Terminator::Branch(target) => {
            if let Some(depth) = loop_target_depth(labels, *target) {
                function.instruction(&Instruction::Br(depth));
            } else {
                let frame = labels
                    .last_mut()
                    .ok_or(EmitError::Internal("arm without an open if"))?;
                if frame.loop_header.is_some() {
                    return Err(EmitError::Internal("arm fall-through over a loop frame"));
                }
                if let Some(existing) = frame.fallthrough {
                    if existing != *target {
                        return Err(EmitError::Internal("if arms disagree on the join"));
                    }
                }
                frame.fallthrough = Some(*target);
            }
        }
        Terminator::BranchIf { cond, then, else_ } => {
            open_if(env, function, labels, tasks, *cond, *then, *else_)?;
        }
    }
    Ok(())
}

/// Finding 13: kind-1 raise whose message operand is the string CELL (the
/// payload word holds the pointer), then `unreachable`.
fn emit_trap_cell(
    env: &EmitEnv,
    function: &mut Function,
    message: InstId,
) -> Result<(), EmitError> {
    let s0 = env.s0();
    function.instruction(&Instruction::LocalGet(env.local_of(message)?));
    function.instruction(&Instruction::LocalSet(s0));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::LocalGet(s0));
    function.instruction(&Instruction::I64Load(mem64(8)));
    function.instruction(&Instruction::I32WrapI64);
    function.instruction(&Instruction::LocalGet(s0));
    function.instruction(&Instruction::I32Load(mem32(4)));
    function.instruction(&Instruction::Call(env.plan.raise_index));
    function.instruction(&Instruction::Unreachable);
    Ok(())
}

/// Depth of `target` when it names an enclosing loop label (`loop_header` or
/// the loop's outer-block join), walking innermost-out; `None` when `target`
/// is not a loop label. Loop frames contribute two labels (loop, block), if
/// frames one.
fn loop_target_depth(labels: &[LabelFrame], target: BlockId) -> Option<u32> {
    let mut depth = 0u32;
    for frame in labels.iter().rev() {
        if frame.loop_header == Some(target) {
            return Some(depth);
        }
        if frame.loop_header.is_some() {
            if frame.join == Some(target) {
                return depth.checked_add(1);
            }
            depth = depth.checked_add(2)?;
        } else {
            depth = depth.checked_add(1)?;
        }
    }
    None
}
