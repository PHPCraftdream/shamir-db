use crate::api::{CompileLimits, Stage};
use crate::back::ir::{
    cell_tag, verify, ArithOp, BlockKind, CastOp, CmpOp, InstKind, IrFunction, Terminator,
    ValueKind,
};
use crate::front::{self, CompileBudget};
use crate::sem;

/// Parses, checks, lowers and verifies one fixture source.
fn lower_source(source: &str) -> crate::back::ir::IrModule {
    let module = front::parse(source, &CompileLimits::default()).expect("fixture source parses");
    let typed = sem::check(&module, &CompileLimits::default(), Stage::Mvp).expect("fixture checks");
    let mut budget =
        CompileBudget::new(&CompileLimits::default()).expect("default limits form a budget");
    let lowered =
        crate::back::lower::lower(&module, &typed, &CompileLimits::default(), &mut budget)
            .expect("fixture lowers");
    // The real verifier is the guard against lowerer/verifier drift.
    verify(&lowered).expect("lowered module verifies");
    lowered
}

fn kinds(function: &IrFunction) -> Vec<&InstKind> {
    function.insts.iter().map(|inst| &inst.kind).collect()
}

#[test]
fn call_order_emits_callees_first_with_the_entry_last() {
    let ir = lower_source(
        "fn add(a: i64, b: i64) -> i64 { return a + b; }\n\
         @entry scalar sum(a: i64, b: i64) -> i64 { return add(a, b); }\n",
    );
    // Derivation: call_order is [add, sum], so functions has 2 entries and
    // the entry (sum) sits at index 1.
    assert_eq!(ir.functions.len(), 2);
    assert_eq!(ir.entry, 1);
    // Derivation (call_order is [add, sum]): functions[0] is `add` (2 params,
    // 3 insts) and functions[1] is the entry `sum` (2 params, Call callee 0).
    // add: LocalGet(0), LocalGet(1), CheckedArith{Add, I64}; the return
    // consumes the sum directly (no LocalSet in a return-only body).
    let add = kinds(&ir.functions[0]);
    assert_eq!(add.len(), 3);
    assert!(matches!(add[0], InstKind::LocalGet(0)));
    assert!(matches!(add[1], InstKind::LocalGet(1)));
    assert!(matches!(
        add[2],
        InstKind::CheckedArith {
            op: ArithOp::Add,
            width: ValueKind::I64,
            ..
        }
    ));
    assert!(matches!(
        ir.functions[0].blocks[0].terminator,
        Terminator::Return(Some(_))
    ));
    // sum: two param reads, then Call{callee 0, 2 args}, then Return.
    let sum = kinds(&ir.functions[1]);
    assert!(matches!(sum[0], InstKind::LocalGet(0)));
    assert!(matches!(sum[1], InstKind::LocalGet(1)));
    assert!(matches!(
        sum[2],
        InstKind::Call { callee: 0, args } if args.len() == 2
    ));
}

#[test]
fn while_break_continue_verify_through_the_real_verifier() {
    let ir = lower_source(
        "@entry scalar probe(c: bool) -> i64 {\n\
         let i: i64 = 0;\n\
         while (i < 10) {\n\
         i = i + 1;\n\
         if (c) { continue; }\n\
         if (i == 7) { break; }\n\
         }\n\
         return i;\n\
         }\n",
    );
    let function = &ir.functions[0];
    // Derivation: one while loop -> exactly one Loop block (region leader).
    let headers: Vec<usize> = function
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| block.kind == BlockKind::Loop)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(headers.len(), 1);
    let header = headers[0] as u32;
    // The body ends with a back edge Branch to the loop header (continue).
    assert!(function.blocks.iter().any(|block| matches!(
        block.terminator,
        Terminator::Branch(target) if target.0 == header
    )));
    // The condition and both ifs produce conditional branches.
    let branch_ifs = function
        .blocks
        .iter()
        .filter(|block| matches!(block.terminator, Terminator::BranchIf { .. }))
        .count();
    assert_eq!(branch_ifs, 3);
    // Break targets a forward block (the loop exit).
    assert!(function.blocks.iter().enumerate().any(|(index, block)| {
        matches!(block.terminator, Terminator::Branch(target) if target.0 > index as u32)
    }));
}

#[test]
fn short_circuit_and_uses_a_temp_local_and_zero_constant() {
    let ir = lower_source("@entry scalar probe(a: bool, b: bool) -> bool { return a && b; }\n");
    let function = &ir.functions[0];
    // Derivation: params a=0, b=1; the temp local is the next slot (2). The
    // false arm stores ConstI32(0) into the temp; the true arm evaluates b
    // (LocalGet(1)) into the temp — `a && b` never materialises a constant 1.
    let zero_set = function.insts.windows(2).any(|window| {
        matches!(&window[0].kind, InstKind::ConstI32(0))
            && matches!(&window[1].kind, InstKind::LocalSet(2))
    });
    assert!(zero_set);
    // Eval arm: the right operand b is read into the temp.
    let eval_set = function.insts.windows(2).any(|window| {
        matches!(&window[0].kind, InstKind::LocalGet(1))
            && matches!(&window[1].kind, InstKind::LocalSet(2))
    });
    assert!(eval_set);
    // lhs test: the BranchIf tests the lowered left value directly (a is
    // param slot 0 — no temp round-trip for the condition).
    let branch_block = function
        .blocks
        .iter()
        .find(|block| matches!(block.terminator, Terminator::BranchIf { .. }))
        .expect("conditional branch block");
    let cond_inst = function
        .insts
        .get(branch_block.start.index() + branch_block.inst_count as usize - 1)
        .expect("condition inst");
    assert!(matches!(&cond_inst.kind, InstKind::LocalGet(0)));
    assert!(function
        .blocks
        .iter()
        .any(|block| matches!(block.terminator, Terminator::BranchIf { .. })));
}

#[test]
fn record_literal_evaluates_fields_in_sorted_name_order() {
    let ir = lower_source(
        "@entry scalar probe() -> i64 {\n\
         let r: { b: i64, a: i64 } = { b: 1, a: 2 };\n\
         return r.a + r.b;\n\
         }\n",
    );
    let function = &ir.functions[0];
    // Derivation: shape fields sorted [a, b]; the literal is stored via two
    // CellSlot copies and exactly one RecordCell describes it.
    let record_cells = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::RecordCell { .. }))
        .count();
    assert_eq!(record_cells, 1);
    assert!(function
        .insts
        .iter()
        .any(|inst| matches!(inst.kind, InstKind::RecordCell { field_count: 2, .. })));
    // CellSlot indices in emission order: the literal writes a (0) then b (1).
    let mut slot_indices = Vec::new();
    for inst in &function.insts {
        if let InstKind::CellSlot { index, .. } = &inst.kind {
            if let InstKind::ConstI32(value) = &function.insts[index.index()].kind {
                slot_indices.push(*value);
            }
        }
    }
    assert_eq!(slot_indices, vec![0, 1]);
    // Field reads use CellElemPtr + CellLoadPayload, a before b.
    let elem_ptr_indices: Vec<i32> = (0..function.insts.len() - 1)
        .filter_map(|index| {
            if let InstKind::CellElemPtr { index: operand, .. } = &function.insts[index].kind {
                if let InstKind::ConstI32(value) = &function.insts[operand.index()].kind {
                    return Some(*value);
                }
            }
            None
        })
        .collect();
    assert_eq!(elem_ptr_indices, vec![0, 1]);
    let loads = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellLoadPayload(_)))
        .count();
    assert_eq!(loads, 2);
}

#[test]
fn list_literal_and_index_use_cell_allocation() {
    let ir = lower_source(
        "@entry scalar probe() -> i64 {\n\
         let xs: [i64] = [3, 4];\n\
         return xs[1];\n\
         }\n",
    );
    let function = &ir.functions[0];
    // Derivation: CellsAlloc(2), two CellCopyTo element stores, one LIST
    // cell with aux = count and payload = the region offset (SExtend64),
    // then CellElemPtr(1) + CellLoadPayload for xs[1].
    assert!(function
        .insts
        .iter()
        .any(|inst| matches!(inst.kind, InstKind::CellsAlloc(_))));
    let copies = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellCopyTo { .. }))
        .count();
    assert_eq!(copies, 2);
    assert!(function.insts.iter().any(|inst| matches!(
        inst.kind,
        InstKind::CellAlloc {
            tag: cell_tag::LIST
        }
    )));
    assert!(function
        .insts
        .iter()
        .any(|inst| matches!(inst.kind, InstKind::CellStoreAux { .. })));
    assert!(function
        .insts
        .iter()
        .any(|inst| matches!(inst.kind, InstKind::CellStorePayload { .. })));
    assert!(function
        .insts
        .iter()
        .any(|inst| matches!(inst.kind, InstKind::CellElemPtr { .. })));
    assert!(function
        .insts
        .iter()
        .any(|inst| matches!(inst.kind, InstKind::CellLoadPayload(_))));
    assert!(function
        .insts
        .iter()
        .any(|inst| matches!(inst.kind, InstKind::SExtend64(_))));
}

#[test]
fn optional_let_boxes_the_scalar_and_builtins_operate_on_the_cell() {
    let ir = lower_source(
        "@entry scalar probe() -> i32? {\n\
         let x: i32? = 5;\n\
         if (is_null(x)) { return null; }\n\
         return unwrap(x);\n\
         }\n",
    );
    let function = &ir.functions[0];
    // Derivation: the checker types the literal 5 as i32, so `let x: i32? = 5`
    // boxes the i32 value into the optional-cell target as
    // CellAllocI64(SExtend64(5)); is_null lowers to CellIsNull, unwrap to
    // CellUnwrap + the i32 payload load; the returned i32 is boxed again for
    // the i32? result.
    assert!(function.insts.windows(2).any(|window| {
        matches!(&window[0].kind, InstKind::SExtend64(_))
            && matches!(&window[1].kind, InstKind::CellAllocI64(_))
    }));
    assert!(function
        .insts
        .iter()
        .any(|inst| matches!(inst.kind, InstKind::CellIsNull(_))));
    assert!(function
        .insts
        .iter()
        .any(|inst| matches!(inst.kind, InstKind::CellUnwrap(_))));
    // The null return is the shared static null cell.
    assert!(function
        .insts
        .iter()
        .any(|inst| matches!(inst.kind, InstKind::StaticCell(_))));
}

#[test]
fn range_for_exits_pre_loop_and_increments_in_its_own_block() {
    let ir = lower_source(
        "@entry scalar probe() -> i64 {\n\
         let total: i64 = 0;\n\
         for (i in 0..5) { total = total + i; }\n\
         return total;\n\
         }\n",
    );
    let function = &ir.functions[0];
    // Derivation: the header only runs the exit test (Compare{Ge, I64}; the
    // bounds and the loop variable are i64, so `total + i` is I64). The body
    // publishes the item and adds once (CheckedArith{Add, I64} #1), the
    // increment block adds 1 (CheckedArith{Add, I64} #2), so exactly two I64
    // adds; the single Loop block is the header and the increment's Branch
    // back to it is the only back edge.
    let ge = function
        .insts
        .iter()
        .filter(|inst| {
            matches!(
                inst.kind,
                InstKind::Compare {
                    op: CmpOp::Ge,
                    width: ValueKind::I64,
                    ..
                }
            )
        })
        .count();
    assert_eq!(ge, 1);
    let adds = function
        .insts
        .iter()
        .filter(|inst| {
            matches!(
                inst.kind,
                InstKind::CheckedArith {
                    op: ArithOp::Add,
                    width: ValueKind::I64,
                    ..
                }
            )
        })
        .count();
    // Body add + increment add.
    assert_eq!(adds, 2);
    let headers: Vec<usize> = function
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| block.kind == BlockKind::Loop)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(headers.len(), 1);
    let header = headers[0] as u32;
    assert!(function.blocks.iter().any(|block| matches!(
        block.terminator,
        Terminator::Branch(target) if target.0 == header
    )));
}

#[test]
fn the_six_cast_builtins_lower_to_their_cast_ops() {
    let ir = lower_source(
        "@entry scalar probe(a: i32) -> f64 {\n\
         let b: i64 = to_i64(a);\n\
         let c: f64 = to_f64_i64(b);\n\
         let d: i64 = to_i64_f64(c);\n\
         let e: i32 = to_i32(d);\n\
         let f: f64 = to_f64_i32(a);\n\
         let g: i32 = to_i32_f64(f);\n\
         return c;\n\
         }\n",
    );
    let kinds = kinds(&ir.functions[0]);
    let expected = [
        CastOp::ToI64FromI32,
        CastOp::ToF64FromI64,
        CastOp::ToI64FromF64,
        CastOp::ToI32FromI64,
        CastOp::ToF64FromI32,
        CastOp::ToI32FromF64,
    ];
    // Derivation: one Cast inst per builtin call, in source order.
    let mut found = 0;
    for kind in &kinds {
        if let InstKind::Cast { op, .. } = kind {
            assert_eq!(*op, expected[found]);
            found += 1;
        }
    }
    assert_eq!(found, expected.len());
}
