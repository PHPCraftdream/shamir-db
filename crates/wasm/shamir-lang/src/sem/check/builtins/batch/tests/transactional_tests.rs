use crate::api::{CompileLimits, Stage};
use crate::sem::tests::helper::check_with_stage;
use crate::sem::{ReferenceKind, TypedModule};

fn check(source: &str) -> Result<TypedModule, Vec<crate::api::Diagnostic>> {
    check_with_stage(source, &CompileLimits::default(), Stage::Procedures)
}

fn clean(source: &str) -> TypedModule {
    check(source).unwrap_or_else(|diagnostics| panic!("expected clean check: {diagnostics:?}"))
}

fn codes(source: &str) -> Vec<&'static str> {
    check(source)
        .expect_err("invalid source")
        .into_iter()
        .map(|diag| diag.code)
        .collect()
}

#[test]
fn batch_tx_has_batch_type_and_requires_zero_arguments() {
    let module = clean("@entry procedure p() -> i64 { let b: Batch = batch_tx(); return 0; }");
    assert!(module.functions.iter().any(|function| function.name == "p"));
    assert_eq!(
        codes("@entry procedure p() -> i64 { let b = batch_tx(1); return 0; }"),
        vec!["T004"]
    );
}

#[test]
fn transaction_bit_flows_through_batch_id_read_insert_update_delete_and_request() {
    let bodies = [
        r#"
    const b0 = batch_read(batch_id(batch_tx(), "tagged"), "src", "users", { id: 1 });
    const value: { id: i64 } = { id: query_ref_field(b0, "src", 0, "id") };
    const b1 = batch_insert(b0, "created", "users", value);
"#,
        r#"
    const b0 = batch_read(batch_tx(), "src", "users", { id: 1 });
    const value: { id: i64 } = { id: query_ref_field(b0, "src", 0, "id") };
    const b1 = batch_read(b0, "dependent", "users", value);
"#,
        r#"
    const initial = batch_insert(batch_tx(), "initial", "users", { id: 1 });
    const b0 = batch_read(initial, "src", "users", { id: 1 });
    const value: { id: i64 } = { id: query_ref_field(b0, "src", 0, "id") };
    const b1 = batch_insert(b0, "created", "users", value);
"#,
        r#"
    const b0 = batch_read(batch_tx(), "src", "users", { id: 1 });
    const value: { id: i64 } = { id: query_ref_field(b0, "src", 0, "id") };
    const updated = batch_update(b0, "updated", "users", { id: 1 }, { name: "x" });
    const b1 = batch_insert(updated, "created", "users", value);
"#,
        r#"
    const b0 = batch_read(batch_tx(), "src", "users", { id: 1 });
    const value: { id: i64 } = { id: query_ref_field(b0, "src", 0, "id") };
    const deleted = batch_delete(b0, "deleted", "users", { id: 1 });
    const b1 = batch_insert(deleted, "created", "users", value);
"#,
    ];
    for body in bodies {
        let source =
            format!("@entry procedure p() -> i64 {{ {body} let req = request(b1); return 0; }}");
        let module = clean(&source);
        let sinks: Vec<_> = module.reference_sinks.iter().flatten().collect();
        assert_eq!(sinks.len(), 1);
        assert!(sinks[0].transactional);
    }
}

#[test]
fn transactional_reference_capability_only_stays_within_procedure_ceiling() {
    let source = r#"
@entry procedure p() -> i64 {
    const b0 = batch_read(batch_tx(), "src", "users", { id: 1 });
    const value: { id: i64 } = { id: query_ref_field(b0, "src", 0, "id") };
    const b1 = batch_insert(b0, "created", "users", value);
    let req = request(b1);
    return 0;
}
"#;
    let module = clean(source);
    let sink = module
        .reference_sinks
        .iter()
        .flatten()
        .next()
        .expect("one sink");
    assert!(sink.transactional);
    assert_eq!(sink.kind, ReferenceKind::I64);
    assert_eq!(
        codes("@entry scalar probe() -> i64 { let b = batch_tx(); return 0; }"),
        vec!["T007"]
    );
    assert_eq!(
        codes("@entry validator v(x: i64) -> Validation { let b = batch_tx(); return accept(); }"),
        vec!["T007"]
    );
}
