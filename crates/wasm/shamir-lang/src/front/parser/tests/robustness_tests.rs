use crate::api::CompileLimits;
use crate::front::{parse, scan};

const SCALAR: &str = r#"fn add(a: i64, b: i64) -> i64 { return a + b; }
@entry scalar sum(a: i64, b: i64) -> i64 { return add(a, b); }"#;
const VALIDATOR: &str = r#"@entry validator require_name(
    record: { name: string }?,
    old_record: { name: string }?
) -> Validation {
    if (is_null(record)) { return record_error("record_required"); }
    let current: { name: string } = unwrap(record);
    if (current.name == "") { return reject("name", "name_empty"); }
    return accept();
}"#;
const HELPERS: &str = r#"fn plus(x: i64) -> i64 { return x + 1; }
fn twice(x: i64) -> i64 { return plus(plus(x)); }
@entry scalar run(x: i64) -> i64 { return twice(x); }"#;

fn outcome(source: &str) -> (Result<usize, String>, Result<String, String>) {
    let scanned = scan(source, &CompileLimits::default())
        .map(|tokens| tokens.len())
        .map_err(|diagnostic| diagnostic.to_string());
    let parsed = parse(source, &CompileLimits::default())
        .map(|module| format!("{module:?}"))
        .map_err(|diagnostic| diagnostic.to_string());
    (scanned, parsed)
}

#[test]
fn every_example_prefix_and_every_single_byte_deletion_never_panics() {
    for example in [SCALAR, VALIDATOR, HELPERS] {
        for end in 0..=example.len() {
            if !example.is_char_boundary(end) {
                continue;
            }
            let prefix = example.get(..end).expect("known character boundary");
            let scan_result = std::panic::catch_unwind(|| scan(prefix, &CompileLimits::default()));
            assert!(
                scan_result.is_ok(),
                "scanner panicked for prefix ending at {end}"
            );
            let parse_result =
                std::panic::catch_unwind(|| parse(prefix, &CompileLimits::default()));
            assert!(
                parse_result.is_ok(),
                "parser panicked for prefix ending at {end}"
            );
        }
        for byte_index in 0..example.len() {
            let mut bytes = example.as_bytes().to_vec();
            bytes.remove(byte_index);
            let Ok(deletion) = String::from_utf8(bytes) else {
                continue;
            };
            let scan_result =
                std::panic::catch_unwind(|| scan(&deletion, &CompileLimits::default()));
            assert!(
                scan_result.is_ok(),
                "scanner panicked deleting byte {byte_index}"
            );
            let parse_result =
                std::panic::catch_unwind(|| parse(&deletion, &CompileLimits::default()));
            assert!(
                parse_result.is_ok(),
                "parser panicked deleting byte {byte_index}"
            );
        }
    }
}

#[test]
fn arbitrary_bounded_utf8_and_hostile_delimiters_are_repeatable_and_panic_free() {
    let hostile = "!".repeat(4096);
    let corpus = [
        "λ🙂\r\n\t\0",
        "@entry scalar f() -> i64 { return ; }",
        "/* nested /* comment */ trailing */",
        "\"é\" b\"\\xFF\"",
        "0x 01 1e+ 1.0e9999",
        "[ { a: [1, 2,], }, ]",
        "&& || .. -> @entryx",
        hostile.as_str(),
    ];
    for source in corpus {
        let scan_first = std::panic::catch_unwind(|| scan(source, &CompileLimits::default()));
        let scan_second = std::panic::catch_unwind(|| scan(source, &CompileLimits::default()));
        assert!(scan_first.is_ok() && scan_second.is_ok());
        let parse_first = std::panic::catch_unwind(|| parse(source, &CompileLimits::default()));
        let parse_second = std::panic::catch_unwind(|| parse(source, &CompileLimits::default()));
        assert!(parse_first.is_ok() && parse_second.is_ok());
        let first = outcome(source);
        let second = outcome(source);
        assert_eq!(first, second);
        assert!(
            first.0.is_ok()
                || first
                    .0
                    .as_ref()
                    .err()
                    .is_some_and(|error| ["L001", "L002", "L003", "B001"]
                        .iter()
                        .any(|code| error.starts_with(code)))
        );
        assert!(
            first.1.is_ok()
                || first.1.as_ref().err().is_some_and(|error| [
                    "L001", "L002", "L003", "P001", "P002", "P003", "B001"
                ]
                .iter()
                .any(|code| error.starts_with(code)))
        );
    }
}

#[test]
fn deterministic_flat_ast_debug_and_diagnostics() {
    let first = parse(HELPERS, &CompileLimits::default()).expect("fixture parses");
    let second = parse(HELPERS, &CompileLimits::default()).expect("same fixture parses");
    assert_eq!(first, second);
    assert_eq!(format!("{first:?}"), format!("{second:?}"));
    let bad = "@entry scalar f() -> i64 { return 1 < 2 < 3; }";
    let diagnostic_a = parse(bad, &CompileLimits::default()).expect_err("invalid expression");
    let diagnostic_b = parse(bad, &CompileLimits::default()).expect_err("same invalid expression");
    assert_eq!(diagnostic_a, diagnostic_b);
    assert_eq!(diagnostic_a.to_string(), diagnostic_b.to_string());

    let flat = parse(
        &format!(
            "@entry scalar f() -> i64 {{ return {}; }}",
            std::iter::repeat_n("x", 120)
                .collect::<Vec<_>>()
                .join(" + ")
        ),
        &CompileLimits::default(),
    )
    .expect("under-cap flat left-associated chain");
    assert!(std::panic::catch_unwind(|| format!("{flat:?}")).is_ok());
}
