//! Spill tests: cross-block operand lowering. Each fixture below was a
//! verifier rejection ("operand from another block") before the spill fix.

use crate::api::{CompileLimits, Stage};
use crate::back::ir::verify;
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
    // The real verifier is the guard against lowerer/verifier drift; every
    // fixture here was a cross-block operand rejection before the fix.
    verify(&lowered).expect("lowered module verifies");
    lowered
}

#[test]
fn call_with_later_conditional_argument_verifies() {
    // Verifier rejection before the fix: the argument 1 spilled (or not)
    // while `c ? 2 : 3` opened blocks before the Call appended in the join.
    lower_source(
        "fn f(x: i64, y: i64) -> i64 { return x + y; }\n\
         @entry scalar probe(c: bool) -> i64 { return f(1, c ? 2 : 3); }\n",
    );
}

#[test]
fn call_with_later_short_circuit_argument_verifies() {
    // Verifier rejection before the fix: `c && true` opened blocks between
    // the first argument's lowering and the Call.
    lower_source(
        "fn g(p: bool, q: i64) -> i64 { return q; }\n\
         @entry scalar probe(c: bool) -> i64 { return g(c && true, 1); }\n",
    );
}

#[test]
fn plain_call_without_block_opening_arguments_still_verifies() {
    // Negative control: compiled and verified before the fix and must stay
    // on the byte-for-byte unchanged spill-free path.
    lower_source(
        "fn f(x: i64, y: i64) -> i64 { return x + y; }\n\
         @entry scalar probe() -> i64 { return f(1, 2); }\n",
    );
}

#[test]
fn arithmetic_with_conditional_right_operand_verifies() {
    // Verifier rejection before the fix: CheckedArith appended in the
    // conditional's join block while ConstI64(1) lived in the entry block.
    lower_source("@entry scalar probe(c: bool) -> i64 { return 1 + (c ? 2 : 3); }\n");
}

#[test]
fn equality_with_conditional_right_operand_verifies() {
    // Verifier rejection before the fix: Compare appended in the join block
    // while the left operand lived in the entry block.
    lower_source("@entry scalar probe(c: bool) -> bool { return 1 == (c ? 2 : 3); }\n");
}

#[test]
fn inequality_with_conditional_right_operand_verifies() {
    // Verifier rejection before the fix: same cross-block Compare shape
    // with CmpOp::Ne.
    lower_source("@entry scalar probe(c: bool) -> bool { return 1 != (c ? 2 : 3); }\n");
}

#[test]
fn ordering_with_conditional_right_operand_verifies() {
    // Verifier rejection before the fix: the ordering Compare appended in
    // the join block while the left operand lived in the entry block.
    lower_source("@entry scalar probe(c: bool) -> bool { return 1 < (c ? 2 : 3); }\n");
}

#[test]
fn list_index_with_conditional_index_verifies() {
    // Verifier rejection before the fix: CellElemPtr appended after the
    // conditional lowered its blocks, while the base lived in the entry.
    lower_source("@entry scalar probe(c: bool, xs: [i64]) -> i64 { return xs[c ? 0 : 1]; }\n");
}

#[test]
fn list_literal_with_conditional_item_verifies() {
    // Verifier rejection before the fix: the CellSlot/SExtend64 consumers
    // referenced the CellsAlloc region from across the conditional blocks.
    lower_source("@entry scalar probe(c: bool) -> [i64] { return [1, c ? 2 : 3]; }\n");
}

#[test]
fn record_literal_with_conditional_field_verifies() {
    // Verifier rejection before the fix: RecordCell (and its CellSlot)
    // referenced the region from across the conditional blocks.
    lower_source("@entry scalar probe(c: bool) -> { v: i64 } { return { v: c ? 2 : 3 }; }\n");
}
