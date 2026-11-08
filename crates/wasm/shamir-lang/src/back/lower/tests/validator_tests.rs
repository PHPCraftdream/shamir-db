use crate::api::{CompileLimits, Stage};
use crate::back::ir::{verify, ArithOp, InstKind, ValueKind};
use crate::front::{self, CompileBudget};
use crate::sem;

/// Parses, checks, lowers and verifies one validator fixture source.
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

/// The milestone-5 design worked example: a validator entry.
const VALIDATOR_EXAMPLE: &str = "@entry validator require_name(\n\
     record: { name: string }?,\n\
     old_record: { name: string }?\n\
     ) -> Validation {\n\
     if (is_null(record)) {\n\
     return record_error(\"record_required\");\n\
     }\n\
     let current: { name: string } = unwrap(record);\n\
     if (current.name == \"\") {\n\
     return reject(\"name\", \"name_empty\");\n\
     }\n\
     return accept();\n\
     }\n";

#[test]
fn validator_entry_marks_entry_is_validator() {
    let ir = lower_source(VALIDATOR_EXAMPLE);
    // Derivation: the checker tags the entry kind as validator and the
    // lowering driver copies it into `IrModule::entry_is_validator`.
    assert!(ir.entry_is_validator);
    // Derivation: a scalar entry must keep the flag false.
    let scalar = lower_source("@entry scalar probe() -> i64 { return 1; }\n");
    assert!(!scalar.entry_is_validator);
}

#[test]
fn accept_lowers_to_empty_list_and_stop_false() {
    let ir = lower_source(
        "@entry validator v(record: { a: i64 }?, old_record: { a: i64 }?) -> Validation { return accept(); }\n",
    );
    let function = &ir.functions[ir.entry as usize];
    // Derivation: accept builds the empty errors list cell (count 0, zero
    // payload — no region), then one 2-slot validation record (RecordCell
    // field_count 2) and one false stop bool cell (CellAllocBool); no append
    // happens, so no CellElemPtr bounds check appears.
    let validation_records = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::RecordCell { field_count: 2, .. }))
        .count();
    assert_eq!(validation_records, 1);
    let bool_cells = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellAllocBool(_)))
        .count();
    assert_eq!(bool_cells, 1);
    let elem_ptrs = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellElemPtr { .. }))
        .count();
    assert_eq!(elem_ptrs, 0);
}

#[test]
fn reject_lowers_field_error_and_single_element_list() {
    let ir = lower_source(
        "@entry validator v(record: { a: i64 }?, old_record: { a: i64 }?) -> Validation { return reject(\"name\", \"name_empty\"); }\n",
    );
    let function = &ir.functions[ir.entry as usize];
    // Derivation: reject builds the ["code","field"] error record
    // (RecordCell field_count 2) and then the ["errors","stop"] validation
    // record (RecordCell field_count 2): exactly two 2-slot records. One
    // CellAllocBool for the false stop; the path string, its one-element
    // list, the error region and the errors region are all CellsAlloc.
    let validation_records = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::RecordCell { field_count: 2, .. }))
        .count();
    assert_eq!(validation_records, 2);
    let bool_cells = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellAllocBool(_)))
        .count();
    assert_eq!(bool_cells, 1);
    let region_allocs = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellsAlloc(_)))
        .count();
    assert!(region_allocs >= 1);
}

#[test]
fn reject_path_keeps_the_path_list() {
    let ir = lower_source(
        "@entry validator v(record: { a: i64 }?, old_record: { a: i64 }?) -> Validation { return reject_path([\"a\", \"b\"], \"nested_code\"); }\n",
    );
    let function = &ir.functions[ir.entry as usize];
    // Derivation: the path list literal allocates its own 2-slot region
    // (lower_list, CellsAlloc #1; the two string cells are static, so no
    // extra region); field_error_cell allocates the 2-slot error region
    // (CellsAlloc #2, RecordCell ["code","field"]); errors_list_single
    // allocates the one-cell errors region (CellsAlloc #3); validation_cell
    // allocates the 2-slot validation region (CellsAlloc #4, RecordCell
    // ["errors","stop"]).
    let region_allocs = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellsAlloc(_)))
        .count();
    assert_eq!(region_allocs, 4);
    let two_slot_records = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::RecordCell { field_count: 2, .. }))
        .count();
    assert_eq!(two_slot_records, 2);
}

#[test]
fn with_error_chain_checks_cap_twice() {
    let ir = lower_source(
        "@entry validator v(record: { a: i64 }?, old_record: { a: i64 }?) -> Validation { return with_error(with_error(accept(), \"a\", \"c1\"), \"b\", \"c2\"); }\n",
    );
    let function = &ir.functions[ir.entry as usize];
    // Derivation: each with_error append performs exactly one CellElemPtr
    // bounds check against the static cap cell (then Drop), and exactly one
    // CheckedArith{Add, I32} computing the grown count; accept itself emits
    // neither. RecordCells: accept's validation record + per with_error one
    // ["code","field"] error record and one new ["errors","stop"] validation
    // record = 1 + 2*2 = 5.
    let elem_ptrs = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellElemPtr { .. }))
        .count();
    assert_eq!(elem_ptrs, 2);
    let record_cells = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::RecordCell { .. }))
        .count();
    assert_eq!(record_cells, 5);
    let grows = function
        .insts
        .iter()
        .filter(|inst| {
            matches!(
                inst.kind,
                InstKind::CheckedArith {
                    op: ArithOp::Add,
                    width: ValueKind::I32,
                    ..
                }
            )
        })
        .count();
    assert_eq!(grows, 2);
    // Derivation: each with_error append copies the old cells once via the
    // runtime copy helper (the first copies 0 cells, the second copies the
    // first error's 1 cell); the instruction count is 2 either way.
    let copies = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellsCopy { .. }))
        .count();
    assert_eq!(copies, 2);
}

#[test]
fn append_copies_into_a_fresh_region() {
    let ir = lower_source(
        "@entry validator v(record: { a: i64 }?, old_record: { a: i64 }?) -> Validation { return with_error(accept(), \"f\", \"c\"); }\n",
    );
    let function = &ir.functions[ir.entry as usize];
    // Derivation: one append performs exactly one CellsCopy — of accept's
    // zero old cells (zero-length memory.copy no-op).
    let copies = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellsCopy { .. }))
        .count();
    assert_eq!(copies, 1);
    // Derivation: accept allocates no list region but still builds a two-slot
    // validation record; the five regions are accept's validation record, the
    // one-slot field list, the two-slot field-error record, the one-slot fresh
    // append region, and the final two-slot validation record.
    let region_allocs = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellsAlloc(_)))
        .count();
    assert_eq!(region_allocs, 5);
}

#[test]
fn stop_preserves_errors_and_sets_stop_true() {
    let ir = lower_source(
        "@entry validator v(record: { a: i64 }?, old_record: { a: i64 }?) -> Validation { return stop(accept()); }\n",
    );
    let function = &ir.functions[ir.entry as usize];
    // Derivation: accept allocates the false stop cell (CellAllocBool #1);
    // stop loads the wrapped validation record's region (CellLoadPayload on
    // the validation cell), reuses its slot-0 errors list, and allocates the
    // true stop cell (CellAllocBool #2) for the rebuilt validation record.
    let bool_cells = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellAllocBool(_)))
        .count();
    assert_eq!(bool_cells, 2);
    let payload_loads = function
        .insts
        .iter()
        .filter(|inst| matches!(inst.kind, InstKind::CellLoadPayload(_)))
        .count();
    assert_eq!(payload_loads, 1);
}
