//! Compile time of `shamir-lang` sources: the four entry kinds and generated modules of
//! growing function counts. The design target for a typical module is about 10 ms.

use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_lang::{compile, CompileOptions, Stage};

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

/// `functions` independent helpers and a scalar entry that sums the first few.
fn generated(functions: usize) -> String {
    let mut source = String::new();
    for index in 0..functions {
        source.push_str(&format!(
            "fn h{index}(x: i64) -> i64 {{ let a: i64 = x * 3 + {index}; if (a > 100) {{ return a - 7; }} return a + 11; }}\n"
        ));
    }
    source.push_str("@entry scalar run(x: i64) -> i64 { return h0(x)");
    for index in 1..functions.min(8) {
        source.push_str(&format!(" + h{index}(x)"));
    }
    source.push_str("; }\n");
    source
}

fn options() -> CompileOptions {
    CompileOptions {
        stage: Stage::Procedures,
        ..CompileOptions::default()
    }
}

pub fn register(h: &mut Harness) {
    let mut cells: Vec<(String, String)> = [
        ("scalar", SCALAR),
        ("validator", VALIDATOR),
        ("procedure", PROCEDURE),
        ("builder", BUILDER),
    ]
    .iter()
    .map(|(id, source)| ((*id).to_string(), (*source).to_string()))
    .collect();
    for functions in [16usize, 64, 100] {
        cells.push((format!("fns_{functions}"), generated(functions)));
    }
    println!("== shamir-lang output sizes ==");
    for (id, source) in &cells {
        let module = compile(source, &options()).expect("source compiles");
        println!(
            "lang/{id}: source={} wasm={}",
            source.len(),
            module.wasm.len()
        );
    }
    for (id, source) in cells {
        h.bench(&format!("lang/compile_{id}"), move || {
            black_box(compile(&source, &options()).expect("source compiles"));
        });
    }
}
