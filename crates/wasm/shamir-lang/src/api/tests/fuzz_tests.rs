//! Deterministic mutation fuzzing of the whole compile pipeline (design
//! section 9, "Property/fuzz suite"): every mutated source either compiles to
//! a valid module or fails with a diagnostic, never panics, and repeats
//! byte for byte.
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::api::{CompileError, CompileOptions, Stage};
use crate::{compile, CompiledModule};

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

const PROCEDURE: &str = r#"@entry procedure p(key: i64) -> i64 {
    let user: { id: i64, name: string }? = db_get("users", key);
    let rows: [{ id: i64 }] = db_query("users", null);
    if (is_null(user)) { return 0; }
    return to_i64(len(rows));
}"#;

const BUILDER: &str = r#"@entry procedure p() -> i64 {
    let b = batch_read(batch(), "u", "users", { role: "admin" });
    let resp = db_execute(request(b));
    let rows: [{ id: i64 }]? = response_rows(resp, "u");
    if (is_null(rows)) { return 0; }
    return to_i64(len(unwrap(rows)));
}"#;

const LITERALS: &str = r#"@entry scalar s(x: i64) -> string {
    let t: string = "a\u{e9}\n\t\"b\"";
    let n: i64 = 9223372036854775807 - x % 7;
    if (n > 0 && x != 3 || !(x < 2)) { return t; }
    return "";
}"#;

const CORPUS: [&str; 5] = [SCALAR, VALIDATOR, PROCEDURE, BUILDER, LITERALS];

/// Fragments that steer mutations toward the grammar's sharp edges.
const SPLICES: [&str; 32] = [
    "{",
    "}",
    "(",
    ")",
    "[",
    "]",
    "\"",
    ";",
    ":",
    ",",
    ".",
    "?",
    "->",
    "==",
    "&&",
    "||",
    "@entry ",
    "fn ",
    "let ",
    "return ",
    "if (",
    " else ",
    "null",
    "0",
    "9223372036854775808",
    "\\u{",
    "/*",
    "//",
    "\u{e9}",
    "\u{0}",
    "-",
    "!",
];

/// xorshift64*: a fixed seed gives the same mutation sequence on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// One to three random edits of `source`, always valid UTF-8.
fn mutate(source: &str, rng: &mut Rng) -> String {
    let mut chars: Vec<char> = source.chars().collect();
    for _ in 0..=rng.below(3) {
        if chars.is_empty() {
            break;
        }
        let at = rng.below(chars.len());
        match rng.below(5) {
            0 => {
                let end = (at + 1 + rng.below(8)).min(chars.len());
                chars.drain(at..end);
            }
            1 => {
                let end = (at + 1 + rng.below(8)).min(chars.len());
                let copy: Vec<char> = chars[at..end].to_vec();
                for (offset, ch) in copy.into_iter().enumerate() {
                    chars.insert(at + offset, ch);
                }
            }
            2 => {
                let splice: Vec<char> = SPLICES[rng.below(SPLICES.len())].chars().collect();
                for (offset, ch) in splice.into_iter().enumerate() {
                    chars.insert(at + offset, ch);
                }
            }
            3 => {
                let splice = SPLICES[rng.below(SPLICES.len())];
                chars[at] = splice.chars().next().expect("splices are not empty");
            }
            _ => chars.truncate(at),
        }
    }
    chars.into_iter().collect()
}

fn options() -> CompileOptions {
    CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    }
}

/// Bytes on success, the debug rendering of the error otherwise.
fn fingerprint(result: &Result<CompiledModule, CompileError>) -> Vec<u8> {
    match result {
        Ok(module) => module.wasm.clone(),
        Err(error) => format!("{error:?}").into_bytes(),
    }
}

#[test]
fn every_corpus_entry_compiles_at_the_procedures_stage() {
    for source in CORPUS {
        compile(source, &options()).expect("corpus entry compiles");
    }
}

#[test]
fn mutated_sources_never_panic_stay_deterministic_and_emit_valid_wasm() {
    let mut compiled = 0usize;
    for (index, source) in CORPUS.iter().enumerate() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (index as u64 + 1));
        for round in 0..200 {
            let mutated = mutate(source, &mut rng);
            let first = catch_unwind(AssertUnwindSafe(|| compile(&mutated, &options())))
                .unwrap_or_else(|_| panic!("compile panicked on corpus {index} round {round}"));
            let second = compile(&mutated, &options());
            assert_eq!(
                fingerprint(&first),
                fingerprint(&second),
                "compile is not deterministic on corpus {index} round {round}"
            );
            if let Ok(module) = first {
                wasmparser::validate(&module.wasm)
                    .unwrap_or_else(|error| panic!("invalid wasm on round {round}: {error}"));
                compiled += 1;
            }
        }
    }
    // The unmutated-looking tail of the distribution must keep some inputs
    // valid, otherwise the loop above only exercises the front end.
    assert!(compiled > 0, "no mutation compiled");
}

#[test]
fn arbitrary_text_never_panics_the_compiler() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for round in 0..300 {
        let length = rng.below(256);
        let bytes: Vec<u8> = (0..length).map(|_| rng.next() as u8).collect();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        catch_unwind(AssertUnwindSafe(|| compile(&text, &options())))
            .unwrap_or_else(|_| panic!("compile panicked on arbitrary text, round {round}"))
            .expect_err("random bytes are not a program");
    }
}

#[test]
fn over_cap_sources_are_rejected_without_panicking() {
    let limit = options().limits.max_source_bytes;
    let oversized = "(".repeat(limit + 1);
    assert!(compile(&oversized, &options()).is_err());
    let deep = "[".repeat(100_000);
    assert!(compile(&deep, &options()).is_err());
    let wide = "a ".repeat(limit / 2);
    assert!(compile(&wide, &options()).is_err());
}
