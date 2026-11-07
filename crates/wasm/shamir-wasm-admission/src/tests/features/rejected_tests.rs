use crate::tests::support::{admit, assemble, limits_with, load, module, module_with_call_body};
use crate::AdmissionError;

const TABLE: &str = "(table 2 funcref)";
/// A function plus a passive element segment `$e` naming it.
const PASSIVE_ELEM: &str = "(func $f) (elem $e func $f)";

const SHARED_MEMORY_WAT: &str = r#"(module
  (memory (export "memory") 1 1 shared)
  (func (export "shamir_alloc") (param i32) (result i32) (i32.const 1024))
  (func (export "shamir_call") (param i32 i32) (result i64) (i64.const 0))
)"#;

const MEMORY64_WAT: &str = r#"(module
  (memory (export "memory") i64 1)
  (func (export "shamir_alloc") (param i32) (result i32) (i32.const 1024))
  (func (export "shamir_call") (param i32 i32) (result i64) (i64.const 0))
)"#;

/// Errors of one module under `Load` and under default `Admission`.
fn both(bytes: &[u8]) -> [AdmissionError; 2] {
    [
        load(bytes).unwrap_err(),
        admit(bytes, limits_with(|_| {})).unwrap_err(),
    ]
}

fn assert_validation_rejected(bytes: &[u8]) {
    for err in both(bytes) {
        assert!(matches!(err, AdmissionError::Validation(_)), "{err}");
    }
}

fn assert_forbidden(extra_items: &str, operator: &str) {
    let bytes = module(extra_items);
    for err in both(&bytes) {
        assert!(
            matches!(err, AdmissionError::ForbiddenOperator { operator: found, .. }
                if found == operator),
            "{err}"
        );
    }
}

#[test]
fn simd_operator_rejected() {
    let bytes = module_with_call_body("", "(drop (v128.const i32x4 0 0 0 0))");
    assert_validation_rejected(&bytes);
}

#[test]
fn threads_shared_memory_rejected() {
    assert_validation_rejected(&assemble(SHARED_MEMORY_WAT));
}

#[test]
fn threads_atomic_operator_rejected() {
    let bytes = module_with_call_body("", "(drop (i32.atomic.load (i32.const 0)))");
    assert_validation_rejected(&bytes);
}

#[test]
fn gc_struct_type_rejected() {
    assert_validation_rejected(&module("(type (struct (field i32)))"));
}

#[test]
fn exceptions_tag_and_throw_rejected() {
    let bytes = module_with_call_body("(tag $t)", "(throw $t)");
    assert_validation_rejected(&bytes);
}

#[test]
fn exceptions_try_table_rejected() {
    let bytes = module_with_call_body("", "(block (try_table (catch_all 0) (nop)))");
    assert_validation_rejected(&bytes);
}

#[test]
fn tail_call_rejected() {
    assert_validation_rejected(&module("(func $t (return_call $t))"));
}

#[test]
fn memory64_rejected() {
    assert_validation_rejected(&assemble(MEMORY64_WAT));
}

#[test]
fn multi_memory_rejected() {
    assert_validation_rejected(&module("(memory 1)"));
}

#[test]
fn externref_global_rejected() {
    let bytes = module("(global externref (ref.null extern))");
    assert_validation_rejected(&bytes);
}

#[test]
fn externref_parameter_rejected() {
    assert_validation_rejected(&module("(func (param externref))"));
}

#[test]
fn second_table_rejected_under_admission_and_load() {
    let bytes = module("(table 1 funcref) (table 1 funcref)");
    for err in both(&bytes) {
        assert!(
            matches!(err, AdmissionError::TooManyTables { observed: 2 }),
            "{err}"
        );
    }
}

#[test]
fn ref_null_in_body_rejected() {
    assert_forbidden("(func (drop (ref.null func)))", "ref.null");
}

#[test]
fn ref_is_null_in_body_rejected() {
    assert_forbidden(
        "(func (param funcref) (drop (ref.is_null (local.get 0))))",
        "ref.is_null",
    );
}

#[test]
fn ref_func_in_body_rejected() {
    assert_forbidden(
        "(func $f (drop (ref.func $f))) (elem declare func $f)",
        "ref.func",
    );
}

#[test]
fn table_get_in_body_rejected() {
    let items = format!("{TABLE} (func (drop (table.get (i32.const 0))))");
    assert_forbidden(&items, "table.get");
}

#[test]
fn table_set_in_body_rejected() {
    let items = format!("{TABLE} (func (param funcref) (table.set (i32.const 0) (local.get 0)))");
    assert_forbidden(&items, "table.set");
}

#[test]
fn table_grow_in_body_rejected() {
    let items =
        format!("{TABLE} (func (param funcref) (drop (table.grow (local.get 0) (i32.const 1))))");
    assert_forbidden(&items, "table.grow");
}

#[test]
fn table_size_in_body_rejected() {
    let items = format!("{TABLE} (func (drop (table.size)))");
    assert_forbidden(&items, "table.size");
}

#[test]
fn table_fill_in_body_rejected() {
    let items = format!(
        "{TABLE} (func (param funcref) (table.fill (i32.const 0) (local.get 0) (i32.const 1)))"
    );
    assert_forbidden(&items, "table.fill");
}

#[test]
fn table_copy_in_body_rejected() {
    let items = format!("{TABLE} (func (table.copy (i32.const 0) (i32.const 0) (i32.const 1)))");
    assert_forbidden(&items, "table.copy");
}

#[test]
fn table_init_in_body_rejected() {
    let items = format!(
        "{TABLE} {PASSIVE_ELEM} (func (table.init $e (i32.const 0) (i32.const 0) (i32.const 1)))"
    );
    assert_forbidden(&items, "table.init");
}

#[test]
fn elem_drop_in_body_rejected() {
    let items = format!("{TABLE} {PASSIVE_ELEM} (func (elem.drop $e))");
    assert_forbidden(&items, "elem.drop");
}

#[test]
fn ref_func_in_active_element_expression_admitted() {
    let items = "(func $f) (table 1 funcref) (elem (i32.const 0) funcref (ref.func $f))";
    let bytes = module(items);
    assert!(load(&bytes).is_ok());
    assert!(admit(&bytes, limits_with(|_| {})).is_ok());
}

#[test]
fn ref_func_in_global_initializer_admitted() {
    let items = "(func $f) (elem declare func $f) (global funcref (ref.func $f))";
    let bytes = module(items);
    assert!(load(&bytes).is_ok());
    assert!(admit(&bytes, limits_with(|_| {})).is_ok());
}

#[test]
fn call_indirect_in_body_admitted() {
    let items = "(type $t (func)) (func $f (type $t)) (table 1 funcref) \
                 (elem (i32.const 0) func $f)";
    let bytes = module_with_call_body(items, "(call_indirect (type $t) (i32.const 0))");
    assert!(load(&bytes).is_ok());
    assert!(admit(&bytes, limits_with(|_| {})).is_ok());
}
