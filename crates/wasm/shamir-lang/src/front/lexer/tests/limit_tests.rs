use crate::api::CompileLimits;
use crate::front::{scan, TokenKind};

fn code(source: &str, limits: &CompileLimits) -> Result<usize, String> {
    scan(source, limits)
        .map(|tokens| tokens.len())
        .map_err(|diagnostic| diagnostic.code.to_owned())
}

#[test]
fn hard_source_single_literal_and_token_ceilings_accept_exact_and_reject_plus_one() {
    let source_exact = " ".repeat(1_048_576);
    let source_over = " ".repeat(1_048_577);
    assert_eq!(code(&source_exact, &CompileLimits::default()), Ok(1));
    assert_eq!(
        code(&source_over, &CompileLimits::default()),
        Err("B001".to_owned())
    );

    let literal_exact = format!("\"{}\"", "a".repeat(65_536));
    let literal_over = format!("\"{}\"", "a".repeat(65_537));
    assert!(scan(&literal_exact, &CompileLimits::default()).is_ok());
    assert_eq!(
        code(&literal_over, &CompileLimits::default()),
        Err("B001".to_owned())
    );
    let token_exact = std::iter::repeat_n("x", 199_999)
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(code(&token_exact, &CompileLimits::default()), Ok(200_000));
    let token_over = std::iter::repeat_n("x", 200_000)
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        code(&token_over, &CompileLimits::default()),
        Err("B001".to_owned())
    );
}

#[test]
fn aggregate_literal_ceiling_and_zero_cap_preflight_are_checked() {
    let source = format!("\"{}\"", "a".repeat(65_536));
    let total_exact = CompileLimits {
        max_total_literal_bytes: 65_536,
        ..CompileLimits::default()
    };
    assert!(scan(&source, &total_exact).is_ok());
    let total_over = CompileLimits {
        max_total_literal_bytes: 65_535,
        ..CompileLimits::default()
    };
    assert_eq!(code(&source, &total_over), Err("B001".to_owned()));
    let zero_source = CompileLimits {
        max_source_bytes: 0,
        ..CompileLimits::default()
    };
    assert_eq!(code("", &zero_source), Ok(1));
    assert_eq!(code(" ", &zero_source), Err("B001".to_owned()));
    let zero_tokens = CompileLimits {
        max_tokens: 0,
        ..CompileLimits::default()
    };
    assert_eq!(code("", &zero_tokens), Err("B001".to_owned()));
    let zero_literal = CompileLimits {
        max_single_literal_bytes: 0,
        ..CompileLimits::default()
    };
    assert_eq!(code("\"\"", &zero_literal), Ok(2));
    assert_eq!(code("1", &zero_literal), Err("B001".to_owned()));
}

#[test]
fn fuel_budget_charges_empty_input_and_whitespace_at_exact_boundary() {
    assert_eq!(
        code(
            " ",
            &CompileLimits {
                compile_fuel: 4,
                ..CompileLimits::default()
            }
        ),
        Ok(1)
    );
    assert_eq!(
        code(
            " ",
            &CompileLimits {
                compile_fuel: 3,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
    assert_eq!(
        code(
            "",
            &CompileLimits {
                compile_fuel: 1,
                ..CompileLimits::default()
            }
        ),
        Ok(1)
    );
    assert_eq!(
        code(
            "",
            &CompileLimits {
                compile_fuel: 0,
                ..CompileLimits::default()
            }
        ),
        Err("B001".to_owned())
    );
}

#[test]
fn small_single_and_aggregate_literal_limits_cover_each_literal_class() {
    let single = CompileLimits {
        max_single_literal_bytes: 2,
        ..CompileLimits::default()
    };
    assert!(scan(r#""ab""#, &single).is_ok());
    assert_eq!(code(r#""abc""#, &single), Err("B001".to_owned()));
    assert!(scan("b\"ab\"", &single).is_ok());
    assert_eq!(code("b\"abc\"", &single), Err("B001".to_owned()));

    let numeric = CompileLimits {
        max_single_literal_bytes: 3,
        ..CompileLimits::default()
    };
    assert!(scan("123", &numeric).is_ok());
    assert_eq!(code("1234", &numeric), Err("B001".to_owned()));

    let aggregate = CompileLimits {
        max_single_literal_bytes: 3,
        max_total_literal_bytes: 3,
        ..CompileLimits::default()
    };
    assert!(scan(r#""a" 12"#, &aggregate).is_ok());
    assert_eq!(code(r#""a" 123"#, &aggregate), Err("B001".to_owned()));
}

#[test]
fn spelling_boundaries_cover_malformed_numbers_and_literal_classes() {
    let valid = scan(
        "0 42 0x7f 2147483647i32 1.00e02f64",
        &CompileLimits::default(),
    )
    .expect("valid literal classes");
    assert!(valid
        .iter()
        .any(|token| matches!(token.kind, TokenKind::Float(value) if value == 100.0)));
    for malformed in [
        "01",
        "0x",
        "1i33",
        "1e",
        "1e+",
        "1.0e9999",
        "0x80000000i32",
        "9223372036854775808",
    ] {
        assert_eq!(
            code(malformed, &CompileLimits::default()),
            Err("L002".to_owned()),
            "{malformed}"
        );
    }
}
