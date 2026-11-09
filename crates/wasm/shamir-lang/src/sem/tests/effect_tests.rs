use crate::api::{CompileLimits, Stage};
use crate::front::EntryKind;
use crate::sem::effects::EffectSet;

use super::helper::{
    check_source, check_with, check_with_stage, entry_of, expect_clean, expect_codes,
    SCALAR_EXAMPLE, VALIDATOR_EXAMPLE,
};

fn validator_entry(body: &str) -> String {
    format!(
        "@entry validator probe(record: {{ name: string }}?, old_record: {{ name: string }}?) -> Validation {{ {body} }}"
    )
}

const HELPER_WITH_CTOR: &str = "fn h() -> i64 { record_error(\"x\"); return 1; }";

#[test]
fn scalar_example_tables_are_deterministic_and_complete() {
    let module = expect_clean(SCALAR_EXAMPLE);
    assert_eq!(module.functions.len(), 2);
    let entry_fn = entry_of(&module);
    assert_eq!(entry_fn.name, "sum");
    assert_eq!(entry_fn.kind, Some(EntryKind::Scalar));
    assert_eq!(module.call_order.len(), 2);
    // The topological order ends at the entry (a function nobody calls);
    // callees come first.
    assert_eq!(module.call_order.last(), Some(&module.entry));
    let add = module
        .functions
        .iter()
        .find(|f| f.name == "add")
        .expect("add is in the typed table");
    assert_eq!(
        add.parameters.iter().map(|p| p.slot).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        entry_fn
            .parameters
            .iter()
            .map(|p| p.slot)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert!(entry_fn.locals.is_empty());
    assert_eq!(entry_fn.effects, EffectSet::new());
    assert_eq!(add.effects, EffectSet::new());
}

#[test]
fn validator_example_tables_record_ctor_effects() {
    let module = expect_clean(VALIDATOR_EXAMPLE);
    assert_eq!(entry_of(&module).kind, Some(EntryKind::Validator));
    assert!(entry_of(&module).effects.validator_ctors());
    let entry_fn = entry_of(&module);
    assert_eq!(
        entry_fn
            .parameters
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["record", "old_record"]
    );
    assert_eq!(
        entry_fn
            .parameters
            .iter()
            .map(|p| p.slot)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(entry_fn.locals.len(), 1);
    let current = &entry_fn.locals[0];
    assert_eq!(current.name, "current");
    assert!(current.mutable);
}

#[test]
fn checking_twice_produces_identical_typed_modules() {
    let first = expect_clean(SCALAR_EXAMPLE);
    let second = expect_clean(SCALAR_EXAMPLE);
    assert_eq!(format!("{:?}", first), format!("{:?}", second));
    let vfirst = expect_clean(VALIDATOR_EXAMPLE);
    let vsecond = expect_clean(VALIDATOR_EXAMPLE);
    assert_eq!(format!("{:?}", vfirst), format!("{:?}", vsecond));
}

#[test]
fn diagnostic_output_is_deterministic() {
    let source = "@entry scalar probe() -> i64 { return missing; return missing; }";
    let first = format!("{:?}", check_source(source));
    let second = format!("{:?}", check_source(source));
    assert_eq!(first, second);
}

#[test]
fn validator_ctors_are_clean_only_in_validators() {
    expect_clean(VALIDATOR_EXAMPLE);
    assert_eq!(
        expect_codes("@entry scalar probe() -> i64 { return reject(\"a\", \"b\"); }"),
        vec!["T007"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe() -> i64 { return accept(); }"),
        vec!["T007"]
    );
}

#[test]
fn validator_ctor_argument_types_are_checked() {
    assert_eq!(
        expect_codes(&validator_entry("return reject(1, false);")),
        vec!["T001", "T001"]
    );
    assert_eq!(
        expect_codes(&validator_entry("return stop(1);")),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes(&validator_entry("return reject_path(\"a\", \"c\");")),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes(&validator_entry("return with_error(\"x\", \"f\", \"c\");")),
        vec!["T001"]
    );
    assert_eq!(
        expect_codes(&validator_entry("return with_record_error(1, \"c\");")),
        vec!["T001"]
    );
    expect_clean(&validator_entry("return reject_path([\"a\"], \"c\");"));
    expect_clean(&validator_entry(
        "return with_error(accept(), \"f\", \"c\");",
    ));
    expect_clean(&validator_entry(
        "return with_record_error(stop(accept()), \"c\");",
    ));
    expect_clean(&validator_entry("return stop(accept());"));
}

#[test]
fn inferred_opaque_aggregates_report_t009() {
    assert_eq!(
        expect_codes(&validator_entry("let xs = [accept()]; return accept();")),
        vec!["T009"]
    );
}

#[test]
fn validators_may_call_pure_builtins() {
    expect_clean(&validator_entry(
        "if (is_null(record) || len(\"x\") == 0 || to_i32(1) != 1) { return reject(\"name\", \"name_empty\"); } return accept();",
    ));
}

#[test]
fn procedure_entries_are_stage_gated() {
    let source = "@entry procedure p(x: i64) -> i64 { return x; }";
    let limits = CompileLimits::default();
    assert_eq!(
        check_with_stage(source, &limits, Stage::Mvp)
            .err()
            .map(|diags| diags.iter().map(|d| d.code).collect::<Vec<_>>()),
        Some(vec!["T007"])
    );
    assert!(check_with_stage(source, &limits, Stage::Procedures).is_ok());
}

#[test]
fn stage2_builtins_are_rejected_everywhere() {
    assert_eq!(
        expect_codes(&validator_entry("return record_error(db_get(\"t\", 1));")),
        vec!["T007"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe() -> i64 { batch(); return 1; }"),
        vec!["T007"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe() -> i64 { request(); return 1; }"),
        vec!["T007"]
    );
}

#[test]
fn helper_ctor_effects_exceed_only_the_scalar_ceiling() {
    assert_eq!(
        expect_codes(&format!(
            "{HELPER_WITH_CTOR}\n@entry scalar probe() -> i64 {{ return h(); }}"
        )),
        vec!["T007"]
    );
    expect_clean(&format!(
        "{HELPER_WITH_CTOR}\n@entry validator probe(record: {{ name: string }}?, old_record: {{ name: string }}?) -> Validation {{ h(); return accept(); }}"
    ));
}

#[test]
fn call_targets_must_be_functions_or_builtins() {
    assert_eq!(
        expect_codes("@entry scalar probe() -> i64 { let f = 1; return f(); }"),
        vec!["T004"]
    );
    assert_eq!(
        expect_codes(
            "fn add(a: i64, b: i64) -> i64 { return a; }\n@entry scalar probe() -> i64 { return add; }"
        ),
        vec!["T004"]
    );
    assert_eq!(
        expect_codes(
            "fn add(a: i64, b: i64) -> i64 { return a; }\n@entry scalar probe() -> i64 { return add(1); }"
        ),
        vec!["T004"]
    );
}

#[test]
fn recursion_is_reported_once_with_a_related_span() {
    let source =
        "fn f(n: i64) -> i64 { return f(n); }\n@entry scalar probe() -> i64 { return f(1); }";
    let diags = check_source(source).expect_err("recursion is rejected");
    let codes: Vec<&'static str> = diags.iter().map(|d| d.code).collect();
    assert_eq!(codes, vec!["T008"]);
    assert!(diags.first().and_then(|d| d.related.as_ref()).is_some());
}

#[test]
fn mutual_recursion_is_reported_exactly_once() {
    let source = "fn f(n: i64) -> i64 { return g(n); }\nfn g(n: i64) -> i64 { return f(n); }\n@entry scalar probe() -> i64 { return f(1); }";
    let diags = check_source(source).expect_err("recursion is rejected");
    let codes: Vec<&'static str> = diags.iter().map(|d| d.code).collect();
    assert_eq!(codes.len(), 1);
    assert_eq!(codes, vec!["T008"]);
    assert!(diags.first().and_then(|d| d.related.as_ref()).is_some());
}

#[test]
fn call_chain_limit_reports_b001() {
    let source = "fn g(a: i64) -> i64 { return a; }\nfn h(a: i64) -> i64 { return g(a); }\n@entry scalar probe() -> i64 { return h(1); }";
    let limits = CompileLimits {
        max_call_chain: 1,
        ..CompileLimits::default()
    };
    let diags = check_with(source, &limits).expect_err("chain is rejected");
    assert!(diags.iter().any(|d| d.code == "B001"));
}

#[test]
fn zero_compile_fuel_reports_b001() {
    let limits = CompileLimits {
        compile_fuel: 0,
        ..CompileLimits::default()
    };
    for source in [SCALAR_EXAMPLE, "@entry scalar probe() -> i64 { return 1; }"] {
        let diags =
            check_with_stage(source, &limits, Stage::Mvp).expect_err("fuel exhaustion is rejected");
        assert!(diags.iter().any(|d| d.code == "B001"));
    }
}

#[test]
fn type_depth_limit_reports_b001() {
    let source = "@entry scalar probe(p: [[i64]]) -> i64 { return 1; }";
    let limits = CompileLimits {
        max_type_depth: 1,
        ..CompileLimits::default()
    };
    // check_with_stage parses with default limits and checks with the lowered
    // ones: the parser rejects the lowered depth first when the same limits
    // are used for parsing, so this is the only way the semantic B001 really
    // runs.
    let diags = check_with_stage(source, &limits, Stage::Mvp).expect_err("depth is rejected");
    assert!(diags.iter().any(|d| d.code == "B001"));
}

#[test]
fn checking_the_scalar_example_with_default_limits_succeeds() {
    let module = check_source(SCALAR_EXAMPLE);
    assert!(module.is_ok());
}

#[test]
fn helper_calling_the_entry_reports_t004() {
    let source = "@entry scalar sum(a: i64, b: i64) -> i64 {\n  return a + b;\n}\nfn shady() -> i64 {\n  return sum(1, 2);\n}";
    let codes: Vec<&str> = expect_codes(source);
    assert!(codes.contains(&"T004"));
}

#[test]
fn hostile_prefixes_and_deletions_never_panic() {
    for source in [SCALAR_EXAMPLE, VALIDATOR_EXAMPLE] {
        for end in 0..=source.len() {
            if let Some(prefix) = source.get(..end) {
                sweep(prefix);
            }
        }
        for index in 0..source.len() {
            if let (Some(head), Some(tail)) = (source.get(..index), source.get(index + 1..)) {
                sweep(&format!("{head}{tail}"));
            }
        }
    }
}

/// Parses and checks one hostile input; any panic fails the test.
fn sweep(source: &str) {
    let limits = CompileLimits::default();
    if let Ok(module) = crate::front::parse(source, &limits) {
        let _ = crate::sem::check(&module, &limits, Stage::Mvp);
    }
}

fn procedure_codes(source: &str) -> Vec<&'static str> {
    match check_with_stage(source, &CompileLimits::default(), Stage::Procedures) {
        Ok(_) => Vec::new(),
        Err(diags) => diags.iter().map(|diag| diag.code).collect(),
    }
}

#[test]
fn procedures_may_use_direct_db_builtins_with_a_contextual_result() {
    // Derivation: the annotation `{...}?` is the expected type of the call,
    // key is a non-null i64 scalar, the table is a string.
    assert!(procedure_codes(
        "@entry procedure p(key: i64) -> i64 { let u: { id: i64, name: string }? = db_get(\"users\", key); if (is_null(u)) { return 0; } return unwrap(u).id; }"
    )
    .is_empty());
    assert!(procedure_codes(
        "@entry procedure p() -> i64 { let rows: [{ id: i64 }] = db_query(\"users\", null); return len(rows) == 0 ? 0 : 1; }"
    )
    .is_empty());
    assert!(procedure_codes(
        "@entry procedure p(name: string) -> { id: i64 } { let stored: { id: i64 } = db_insert(\"users\", { name: name }); return stored; }"
    )
    .is_empty());
}

#[test]
fn direct_db_builtins_need_a_contextual_result_type() {
    // Derivation: an unannotated initializer has no expected type.
    assert_eq!(
        procedure_codes(
            "@entry procedure p(key: i64) -> i64 { let u = db_get(\"users\", key); return 0; }"
        ),
        vec!["T010"]
    );
    // An optional-record context is required by db_get, a record by db_insert.
    assert_eq!(
        procedure_codes(
            "@entry procedure p(key: i64) -> i64 { let u: { id: i64 } = db_get(\"users\", key); return 0; }"
        ),
        vec!["T010"]
    );
    assert_eq!(
        procedure_codes(
            "@entry procedure p() -> i64 { let u: { id: i64 }? = db_insert(\"users\", { id: 1 }); return 0; }"
        ),
        vec!["T010"]
    );
}

#[test]
fn direct_db_builtin_argument_rules() {
    // A list-valued predicate field is not a scalar record.
    assert_eq!(
        procedure_codes(
            "@entry procedure p() -> i64 { let rows: [{ id: i64 }] = db_query(\"users\", { ids: [1] }); return 0; }"
        ),
        vec!["T001"]
    );
    // The table must be a string.
    assert_eq!(
        procedure_codes(
            "@entry procedure p() -> i64 { let rows: [{ id: i64 }] = db_query(1, null); return 0; }"
        ),
        vec!["T001"]
    );
}

#[test]
fn direct_db_builtins_exceed_scalar_and_validator_ceilings() {
    let call = "let rows: [{ id: i64 }] = db_query(\"users\", null);";
    assert_eq!(
        procedure_codes(&format!(
            "@entry scalar probe() -> i64 {{ {call} return 1; }}"
        )),
        vec!["T007"]
    );
    // A helper with database effects is allowed under a procedure entry only.
    let helper = format!("fn h() -> i64 {{ {call} return to_i64(len(rows)); }}");
    assert!(procedure_codes(&format!(
        "{helper}\n@entry procedure p() -> i64 {{ return h(); }}"
    ))
    .is_empty());
    assert_eq!(
        procedure_codes(&format!(
            "{helper}\n@entry scalar probe() -> i64 {{ return h(); }}"
        )),
        vec!["T007"]
    );
}

#[test]
fn procedures_may_use_the_typed_builder_intrinsics() {
    // Derivation: every call matches its fixed opaque signature, the record
    // literal filter is a scalar record and the response projection has an
    // optional-list-of-records context.
    assert!(procedure_codes(
        "@entry procedure p() -> i64 { let b = batch_id(batch(), \"r\"); let b2 = batch_read(b, \"u\", \"users\", { role: \"admin\" }); let b3 = batch_insert(b2, \"i\", \"users\", { name: \"Zed\" }); let resp = db_execute(request(b3)); let rows: [{ id: i64 }]? = response_rows(resp, \"u\"); if (is_null(rows)) { return 0; } return to_i64(len(unwrap(rows))); }"
    )
    .is_empty());
}

#[test]
fn builder_intrinsic_typing_errors() {
    // The response projection needs its contextual optional list of records.
    assert_eq!(
        procedure_codes(
            "@entry procedure p() -> i64 { let resp = db_execute(request(batch())); let rows = response_rows(resp, \"u\"); return 0; }"
        ),
        vec!["T010"]
    );
    // A list-valued filter field is not a scalar record.
    assert_eq!(
        procedure_codes(
            "@entry procedure p() -> i64 { let b = batch_read(batch(), \"u\", \"users\", { ids: [1] }); return 0; }"
        ),
        vec!["T001"]
    );
    // `db_execute` takes a Request, not a Batch.
    assert_eq!(
        procedure_codes(
            "@entry procedure p() -> i64 { let resp = db_execute(batch()); return 0; }"
        ),
        vec!["T001"]
    );
    // Scalar entries may not build batches.
    assert_eq!(
        procedure_codes("@entry scalar probe() -> i64 { let b = batch(); return 1; }"),
        vec!["T007"]
    );
}
