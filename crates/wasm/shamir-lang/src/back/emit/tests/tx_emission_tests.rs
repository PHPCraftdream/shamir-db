use crate::api::{CompileLimits, Stage};
use crate::back::emit::module::emit_compiled_module;
use crate::back::ir::IrModule;
use crate::front;
use crate::front::CompileBudget;

use wasmparser::{Parser, Payload};

const NON_TRANSACTIONAL: &str =
    "@entry procedure p() -> i64 { let response = db_execute(request(batch())); return 0; }";
const NON_TRANSACTIONAL_WITH_UNREACHABLE_BATCH_TX: &str = "fn unreachable_tx_helper() -> i64 {\n    let response = db_execute(request(batch_tx()));\n    return 0;\n}\n@entry procedure p() -> i64 {\n    let response = db_execute(request(batch()));\n    return 0;\n}";
const TRANSACTIONAL: &str =
    "@entry procedure p() -> i64 { let response = db_execute(request(batch_tx())); return 0; }";

fn compile(source: &str) -> (Vec<u8>, IrModule) {
    let limits = CompileLimits::default();
    let module = front::parse(source, &limits).expect("fixture parses");
    let typed = crate::sem::check(&module, &limits, Stage::Procedures).expect("fixture checks");
    let mut budget = CompileBudget::new(&limits).expect("default compile budget");
    let ir =
        crate::back::lower::lower(&module, &typed, &limits, &mut budget).expect("fixture lowers");
    let mut budget = CompileBudget::new(&limits).expect("default compile budget");
    let bytes = emit_compiled_module(&ir, &limits, &mut budget).expect("fixture emits");
    (bytes, ir)
}

fn defined_function_count(bytes: &[u8]) -> u32 {
    Parser::new(0)
        .parse_all(bytes)
        .find_map(|payload| match payload.expect("emitted module parses") {
            Payload::FunctionSection(section) => Some(section.count()),
            _ => None,
        })
        .expect("function section is present")
}

fn static_text_occurrences(bytes: &[u8], needle: &[u8]) -> usize {
    Parser::new(0)
        .parse_all(bytes)
        .filter_map(|payload| match payload.ok()? {
            Payload::DataSection(section) => Some(section),
            _ => None,
        })
        .flat_map(|section| section.into_iter())
        .filter_map(Result::ok)
        .map(|segment| {
            segment
                .data
                .windows(needle.len())
                .filter(|window| *window == needle)
                .count()
        })
        .sum()
}

fn has_static_text(bytes: &[u8], needle: &[u8]) -> bool {
    static_text_occurrences(bytes, needle) != 0
}

/// These strings are exclusive to the tx response checker (the ordinary
/// BatchRequest descriptor already contains the unrelated `transactional`).
const TX_ONLY_STRINGS: [&[u8]; 11] = [
    b"status",
    b"reason",
    b"committed",
    b"aborted",
    b"indeterminate",
    b"skip_aborted",
    b"tx_conflict",
    b"batch reference did not resolve to the expected scalar",
    b"transactional batch aborted by a conflict",
    b"transactional batch aborted",
    b"transactional batch outcome is unknown",
];

#[test]
fn ordinary_batch_execute_does_not_register_or_bake_transaction_check() {
    let (bytes, ir) = compile(NON_TRANSACTIONAL);
    assert_eq!(static_text_occurrences(&bytes, b"transaction"), 1);
    assert!(!ir.batch_uses().transactional);
    for text in TX_ONLY_STRINGS {
        assert!(
            !has_static_text(&bytes, text),
            "non-transactional module must not bake {text:?}"
        );
    }
    // Existing typed-builder emission: ten codec helpers, three runtime
    // functions, one user function, the bytes equality helper and db_execute.
    assert_eq!(defined_function_count(&bytes), 16);
}

#[test]
fn unreachable_batch_tx_helper_does_not_change_emitted_entry_bytes() {
    let (baseline_bytes, baseline_ir) = compile(NON_TRANSACTIONAL);
    let (unreachable_bytes, unreachable_ir) = compile(NON_TRANSACTIONAL_WITH_UNREACHABLE_BATCH_TX);

    // The lowerer emits only call-graph-reachable functions; the declared
    // helper is valid source but absent from the emitted IR and its BatchUses.
    assert_eq!(baseline_ir.functions.len(), 1);
    assert_eq!(unreachable_ir.functions.len(), 1);
    assert!(!baseline_ir.batch_uses().transactional);
    assert!(!unreachable_ir.batch_uses().transactional);
    // This locks reachability laziness against an independently compiled
    // source without the helper. It is not a historical byte fixture: both
    // outputs use the current emitter.
    assert_eq!(baseline_bytes, unreachable_bytes);
}

#[test]
fn batch_tx_registers_its_lazy_response_checker() {
    let (baseline_bytes, _) = compile(NON_TRANSACTIONAL);
    let (bytes, ir) = compile(TRANSACTIONAL);
    assert!(
        static_text_occurrences(&bytes, b"transaction")
            > static_text_occurrences(&baseline_bytes, b"transaction"),
        "transactional checking adds transaction-specific static text"
    );
    assert!(ir.batch_uses().transactional);
    for text in TX_ONLY_STRINGS {
        assert!(
            has_static_text(&bytes, text),
            "transactional module must bake {text:?}"
        );
    }
    // In addition to the ordinary db_execute helper, tx reachability adds a
    // map finder and the (request, response) checker.
    assert_eq!(defined_function_count(&bytes), 18);
}
