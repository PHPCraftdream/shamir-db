use super::helper::{expect_clean, expect_codes};

fn entry(body: &str) -> String {
    format!("@entry scalar probe(c: bool) -> i64 {{ {body} }}")
}

#[test]
fn one_branch_return_without_fallthrough_reports_t005() {
    assert_eq!(expect_codes(&entry("if (c) { return 1; }")), vec!["T005"]);
}

#[test]
fn trailing_return_after_one_branch_if_is_clean() {
    expect_clean(&entry("if (c) { return 1; } return 0;"));
}

#[test]
fn both_branches_returning_is_clean() {
    expect_clean(&entry("if (c) { return 1; } else { return 2; }"));
}

#[test]
fn else_if_chain_with_returning_leaves_is_clean() {
    expect_clean(&entry(
        "if (c) { return 1; } else { if (!c) { return 2; } else { return 3; } }",
    ));
}

#[test]
fn loop_return_does_not_establish_completion() {
    assert_eq!(
        expect_codes(&entry("while (c) { return 1; }")),
        vec!["T005"]
    );
}

#[test]
fn throw_terminates_a_path() {
    expect_clean(&entry("if (c) { return 1; } throw \"x\";"));
}

#[test]
fn null_function_return_rules() {
    expect_clean("@entry scalar probe() -> i64 { return 0; }\nfn tail() -> null { return; }");
    expect_clean(
        "@entry scalar probe() -> i64 { return 0; }\nfn tail() -> null { return; throw \"e\"; }",
    );
    assert_eq!(
        expect_codes("@entry scalar probe() -> i64 { return; }"),
        vec!["T005"]
    );
    assert_eq!(
        expect_codes("@entry scalar probe() -> i64 { return 0; }\nfn tail() -> null { return 1; }"),
        vec!["T001"]
    );
}

#[test]
fn throw_requires_a_string_value() {
    assert_eq!(expect_codes(&entry("throw 5;")), vec!["T001"]);
}

#[test]
fn a_function_that_only_throws_is_complete() {
    expect_clean(&entry("throw \"x\";"));
}

#[test]
fn break_and_continue_outside_loops_report_t005() {
    assert_eq!(expect_codes(&entry("break;")), vec!["T005"]);
    assert_eq!(expect_codes(&entry("continue;")), vec!["T005"]);
}

#[test]
fn break_and_continue_inside_while_are_clean() {
    expect_clean(&entry("while (c) { break; } return 0;"));
    expect_clean(&entry("while (c) { continue; } return 0;"));
}

#[test]
fn break_and_continue_inside_range_for_are_clean() {
    expect_clean(&entry("for (i in 0..10) { break; } return 0;"));
    expect_clean(&entry("for (i in 0..10) { continue; } return 0;"));
}

#[test]
fn break_inside_list_for_is_clean() {
    expect_clean(&entry(
        "let xs: [i64] = [1]; for (x in xs) { break; } return 0;",
    ));
}

#[test]
fn loop_return_falls_through_when_the_loop_runs_zero_times() {
    assert_eq!(
        expect_codes(&entry("for (i in 0..10) { return i; }")),
        vec!["T005"]
    );
    assert_eq!(
        expect_codes(&entry("let xs: [i64] = [1]; for (x in xs) { return x; }")),
        vec!["T005"]
    );
}

#[test]
fn range_bound_type_rules() {
    expect_clean(&entry("for (i in 0..10) { } return 0;"));
    expect_clean("@entry scalar probe(p: i32, q: i32) -> i64 { for (i in p..q) { } return 0; }");
    assert_eq!(
        expect_codes(
            "@entry scalar probe(p: i32, q: i64) -> i64 { for (i in p..q) { } return 0; }"
        ),
        vec!["T003"]
    );
    assert_eq!(
        expect_codes(&entry("for (i in \"a\"..\"b\") { } return 0;")),
        vec!["T003"]
    );
}

#[test]
fn loop_variables_are_immutable() {
    assert_eq!(
        expect_codes(&entry("for (i in 0..10) { i = 1; } return 0;")),
        vec!["T006"]
    );
    assert_eq!(
        expect_codes(&entry(
            "let xs: [i64] = [1]; for (x in xs) { x = 1; } return 0;"
        )),
        vec!["T006"]
    );
}

#[test]
fn unreachable_statements_are_still_type_checked() {
    assert_eq!(
        expect_codes(&entry("{ return 1; return \"x\"; }")),
        vec!["T001"]
    );
}
