use crate::api::CompileLimits;
use crate::api::Span;
use crate::back::ir::module::IrBlock;
use crate::back::ir::verifier::VerifierError;
use crate::back::ir::verify;
use crate::back::ir::ArithOp;
use crate::back::ir::BlockId;
use crate::back::ir::BlockKind;
use crate::back::ir::CmpOp;
use crate::back::ir::Inst;
use crate::back::ir::InstId;
use crate::back::ir::InstKind;
use crate::back::ir::IrBuilder;
use crate::back::ir::IrFunction;
use crate::back::ir::IrLimits;
use crate::back::ir::IrModule;
use crate::back::ir::Terminator;
use crate::back::ir::ValueKind;
use crate::front::CompileBudget;

fn span() -> Span {
    Span::source_start()
}

fn generous_limits() -> IrLimits {
    IrLimits {
        max_ir_nodes: usize::MAX,
        max_ir_nodes_per_function: usize::MAX,
        max_single_literal_bytes: usize::MAX,
        max_total_literal_bytes: usize::MAX,
        max_static_bytes: usize::MAX,
    }
}

fn inst(kind: InstKind) -> Inst {
    Inst { kind, span: span() }
}

fn block(
    kind: BlockKind,
    start: u32,
    inst_count: u32,
    terminator: Terminator,
    region: Option<u32>,
) -> IrBlock {
    IrBlock {
        kind,
        start: InstId(start),
        inst_count,
        terminator,
        region,
    }
}

fn function(
    params: Vec<ValueKind>,
    result: Option<ValueKind>,
    locals: Vec<ValueKind>,
    blocks: Vec<IrBlock>,
    insts: Vec<Inst>,
) -> IrFunction {
    IrFunction {
        params,
        result,
        locals,
        blocks,
        insts,
        param_projections: Vec::new(),
        result_projection: None,
        param_names: Vec::new(),
        param_type_nodes: Vec::new(),
    }
}

fn module(functions: Vec<IrFunction>) -> IrModule {
    IrModule {
        functions,
        entry: 0,
        static_bytes: Vec::new(),
        eq_shapes: Vec::new(),
        type_nodes: Vec::new(),
    }
}

fn expect_variant(actual: VerifierError, expected: VerifierError, label: &str) {
    // Only the variant discriminant matters in these hand-built negatives.
    assert_eq!(
        std::mem::discriminant(&actual),
        std::mem::discriminant(&expected),
        "{}: expected {:?}, got {:?}",
        label,
        expected,
        actual
    );
}

#[test]
fn rejects_operand_from_another_block() {
    let function = function(
        vec![],
        None,
        vec![],
        vec![
            block(BlockKind::Plain, 0, 1, Terminator::Branch(BlockId(1)), None),
            block(BlockKind::Plain, 1, 1, Terminator::Return(None), None),
        ],
        vec![
            inst(InstKind::ConstI32(1)),
            inst(InstKind::BoolNot(InstId(0))),
        ],
    );
    let error = verify(&module(vec![function])).expect_err("cross-block operand");
    expect_variant(
        error,
        VerifierError::OperandNotInBlock {
            function: 0,
            block: 0,
            inst: 0,
            detail: String::new(),
        },
        "cross-block operand",
    );
}

#[test]
fn rejects_local_set_kind_mismatch() {
    let function = function(
        vec![ValueKind::I64],
        None,
        vec![],
        vec![block(
            BlockKind::Plain,
            0,
            2,
            Terminator::Return(None),
            None,
        )],
        vec![inst(InstKind::ConstI32(0)), inst(InstKind::LocalSet(0))],
    );
    let error = verify(&module(vec![function])).expect_err("set kind mismatch");
    expect_variant(
        error,
        VerifierError::KindMismatch {
            function: 0,
            block: 0,
            inst: 0,
            detail: String::new(),
        },
        "set kind mismatch",
    );
}

#[test]
fn rejects_return_value_kind_mismatch() {
    let function = function(
        vec![],
        Some(ValueKind::I64),
        vec![],
        vec![block(
            BlockKind::Plain,
            0,
            1,
            Terminator::Return(Some(InstId(0))),
            None,
        )],
        vec![inst(InstKind::ConstI32(0))],
    );
    let error = verify(&module(vec![function])).expect_err("return kind mismatch");
    expect_variant(
        error,
        VerifierError::KindMismatch {
            function: 0,
            block: 0,
            inst: 0,
            detail: String::new(),
        },
        "return kind mismatch",
    );
}

#[test]
fn rejects_branch_backward_to_a_plain_block() {
    // Block 0 branches to itself: not forward and not a loop header.
    let function = function(
        vec![],
        None,
        vec![],
        vec![
            block(BlockKind::Plain, 0, 0, Terminator::Branch(BlockId(0)), None),
            block(BlockKind::Plain, 0, 0, Terminator::Return(None), None),
        ],
        vec![],
    );
    let error = verify(&module(vec![function])).expect_err("backward plain branch");
    expect_variant(
        error,
        VerifierError::BadBranchDirection {
            function: 0,
            block: 0,
            target: 0,
        },
        "backward plain branch",
    );
}

#[test]
fn rejects_branch_if_cond_that_is_not_i32() {
    let function = function(
        vec![],
        None,
        vec![],
        vec![
            block(
                BlockKind::Plain,
                0,
                1,
                Terminator::BranchIf {
                    cond: InstId(0),
                    then: BlockId(1),
                    else_: BlockId(1),
                },
                None,
            ),
            block(BlockKind::Plain, 1, 0, Terminator::Return(None), None),
        ],
        vec![inst(InstKind::ConstF64(0))],
    );
    let error = verify(&module(vec![function])).expect_err("cond kind mismatch");
    expect_variant(
        error,
        VerifierError::KindMismatch {
            function: 0,
            block: 0,
            inst: 0,
            detail: String::new(),
        },
        "cond kind mismatch",
    );
}

#[test]
fn rejects_non_contiguous_region() {
    // Region 0 spans blocks 0 and 2, but block 1 (no region) sits between.
    let function = function(
        vec![],
        None,
        vec![],
        vec![
            block(
                BlockKind::Loop,
                0,
                0,
                Terminator::Branch(BlockId(1)),
                Some(0),
            ),
            block(BlockKind::Plain, 0, 0, Terminator::Branch(BlockId(2)), None),
            block(BlockKind::Plain, 0, 0, Terminator::Return(None), Some(0)),
        ],
        vec![],
    );
    let error = verify(&module(vec![function])).expect_err("region gap");
    expect_variant(
        error,
        VerifierError::RegionNotContiguous {
            function: 0,
            region: 0,
            detail: String::new(),
        },
        "region gap",
    );
}

#[test]
fn rejects_unreachable_block() {
    let function = function(
        vec![],
        None,
        vec![],
        vec![
            block(BlockKind::Plain, 0, 0, Terminator::Return(None), None),
            block(BlockKind::Plain, 0, 0, Terminator::Return(None), None),
        ],
        vec![],
    );
    let error = verify(&module(vec![function])).expect_err("unreachable block");
    expect_variant(
        error,
        VerifierError::UnreachableBlock {
            function: 0,
            block: 0,
        },
        "unreachable block",
    );
}

#[test]
fn rejects_blocks_that_do_not_tile_the_arena() {
    // Block 1 must start at inst 1 (where block 0 ends), not at inst 2.
    let function = function(
        vec![],
        Some(ValueKind::I32),
        vec![],
        vec![
            block(
                BlockKind::Plain,
                0,
                1,
                Terminator::Return(Some(InstId(0))),
                None,
            ),
            block(BlockKind::Plain, 2, 0, Terminator::Return(None), None),
        ],
        vec![inst(InstKind::ConstI32(0))],
    );
    let error = verify(&module(vec![function])).expect_err("tiling gap");
    expect_variant(
        error,
        VerifierError::BlockRangeTile {
            function: 0,
            block: 0,
            detail: String::new(),
        },
        "tiling gap",
    );
}

#[test]
fn rejects_local_get_without_a_dominating_set() {
    // Slot 0 (a local) is set only in block 1, which does not dominate block 0.
    let function = function(
        vec![],
        None,
        vec![ValueKind::I32],
        vec![
            block(BlockKind::Plain, 0, 1, Terminator::Branch(BlockId(1)), None),
            block(BlockKind::Plain, 1, 2, Terminator::Return(None), None),
        ],
        vec![
            inst(InstKind::LocalGet(0)),
            inst(InstKind::ConstI32(0)),
            inst(InstKind::LocalSet(0)),
        ],
    );
    let error = verify(&module(vec![function])).expect_err("read before set");
    expect_variant(
        error,
        VerifierError::LocalNotDominated {
            function: 0,
            block: 0,
            inst: 0,
            slot: 0,
        },
        "read before set",
    );
}

#[test]
fn rejects_callee_index_out_of_range() {
    let function = function(
        vec![],
        None,
        vec![],
        vec![block(
            BlockKind::Plain,
            0,
            1,
            Terminator::Return(None),
            None,
        )],
        vec![inst(InstKind::Call {
            callee: 5,
            args: Vec::new(),
        })],
    );
    let error = verify(&module(vec![function])).expect_err("callee out of range");
    expect_variant(
        error,
        VerifierError::OutOfRangeIds {
            function: 0,
            block: 0,
            inst: 0,
            detail: String::new(),
        },
        "callee out of range",
    );
}

#[test]
fn rejects_slot_out_of_range() {
    let function = function(
        vec![],
        None,
        vec![],
        vec![block(
            BlockKind::Plain,
            0,
            1,
            Terminator::Return(None),
            None,
        )],
        vec![inst(InstKind::LocalGet(7))],
    );
    let error = verify(&module(vec![function])).expect_err("slot out of range");
    expect_variant(
        error,
        VerifierError::OutOfRangeIds {
            function: 0,
            block: 0,
            inst: 0,
            detail: String::new(),
        },
        "slot out of range",
    );
}

#[test]
fn accepts_add_and_while_loop_shapes() {
    let mut budget = CompileBudget::new(&CompileLimits::default()).expect("default limits");
    let mut builder = IrBuilder::new(generous_limits(), &mut budget);
    // add(a: I64, b: I64) -> I64
    builder
        .begin_function(
            vec![ValueKind::I64, ValueKind::I64],
            Some(ValueKind::I64),
            Vec::new(),
        )
        .expect("begin add");
    builder.open_block(None).expect("entry block");
    let a = builder
        .append(InstKind::LocalGet(0), span())
        .expect("get a");
    let b = builder
        .append(InstKind::LocalGet(1), span())
        .expect("get b");
    let sum = builder
        .append(
            InstKind::CheckedArith {
                op: ArithOp::Add,
                width: ValueKind::I64,
                left: a,
                right: b,
            },
            span(),
        )
        .expect("add");
    builder.terminate_return(Some(sum)).expect("return");
    builder.finish_function().expect("finish add");
    // count(): a while loop with continue (back edge) and a forward exit.
    builder
        .begin_function(vec![], None, vec![ValueKind::I32])
        .expect("begin count");
    let init = builder.open_block(None).expect("init block");
    let _zero = builder.append(InstKind::ConstI32(0), span()).expect("zero");
    builder
        .append(InstKind::LocalSet(0), span())
        .expect("init counter");
    let header = builder.open_loop(0).expect("loop header");
    builder.set_current(init).expect("back to init");
    builder.terminate_branch(header).expect("enter loop");
    builder.set_current(header).expect("emit the condition");
    let get = builder.append(InstKind::LocalGet(0), span()).expect("get");
    let three = builder
        .append(InstKind::ConstI32(3), span())
        .expect("three");
    let cond = builder
        .append(
            InstKind::Compare {
                op: CmpOp::Lt,
                width: ValueKind::I32,
                left: get,
                right: three,
            },
            span(),
        )
        .expect("cond");
    let body = builder.open_block(None).expect("body block");
    builder.set_current(header).expect("back to the header");
    builder
        .terminate_branch_if(cond, body, BlockId(3))
        .expect("loop branch");
    builder.set_current(body).expect("emit the body");
    let get = builder.append(InstKind::LocalGet(0), span()).expect("get");
    let one = builder.append(InstKind::ConstI32(1), span()).expect("one");
    let _next = builder
        .append(
            InstKind::CheckedArith {
                op: ArithOp::Add,
                width: ValueKind::I32,
                left: get,
                right: one,
            },
            span(),
        )
        .expect("increment");
    builder
        .append(InstKind::LocalSet(0), span())
        .expect("store");
    // continue: a back edge to the loop header is allowed.
    builder.terminate_branch(BlockId(1)).expect("continue");
    let exit = builder.open_block(None).expect("exit block");
    assert_eq!(exit, BlockId(3));
    builder.terminate_return(None).expect("exit return");
    builder.finish_function().expect("finish count");
    let built = builder.finish().expect("module");
    verify(&built).expect("while-loop shape verifies");
}

#[test]
fn rejects_instruction_that_references_itself() {
    let function = function(
        vec![],
        None,
        vec![],
        vec![block(
            BlockKind::Plain,
            0,
            1,
            Terminator::Return(None),
            None,
        )],
        vec![inst(InstKind::BoolNot(InstId(0)))],
    );
    let error = verify(&module(vec![function])).expect_err("self reference");
    expect_variant(
        error,
        VerifierError::OperandNotInBlock {
            function: 0,
            block: 0,
            inst: 0,
            detail: String::new(),
        },
        "self reference",
    );
}

#[test]
fn rejects_instruction_that_references_a_later_one() {
    let function = function(
        vec![],
        None,
        vec![],
        vec![block(
            BlockKind::Plain,
            0,
            2,
            Terminator::Return(None),
            None,
        )],
        vec![
            inst(InstKind::BoolNot(InstId(1))),
            inst(InstKind::ConstI32(1)),
        ],
    );
    let error = verify(&module(vec![function])).expect_err("forward reference");
    expect_variant(
        error,
        VerifierError::OperandNotInBlock {
            function: 0,
            block: 0,
            inst: 0,
            detail: String::new(),
        },
        "forward reference",
    );
}

#[test]
fn accepts_local_set_in_both_arms_before_the_join_read() {
    // Slot 0 is initialised on both arms of the branch, so the join read is
    // dominated by a set on every path.
    let function = function(
        vec![],
        None,
        vec![ValueKind::I32],
        vec![
            block(
                BlockKind::Plain,
                0,
                1,
                Terminator::BranchIf {
                    cond: InstId(0),
                    then: BlockId(1),
                    else_: BlockId(2),
                },
                None,
            ),
            block(BlockKind::Plain, 1, 2, Terminator::Branch(BlockId(3)), None),
            block(BlockKind::Plain, 3, 2, Terminator::Branch(BlockId(3)), None),
            block(BlockKind::Plain, 5, 1, Terminator::Return(None), None),
        ],
        vec![
            inst(InstKind::ConstI32(1)),
            inst(InstKind::ConstI32(5)),
            inst(InstKind::LocalSet(0)),
            inst(InstKind::ConstI32(6)),
            inst(InstKind::LocalSet(0)),
            inst(InstKind::LocalGet(0)),
        ],
    );
    verify(&module(vec![function])).expect("both arms initialise the slot");
}
