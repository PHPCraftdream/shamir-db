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
