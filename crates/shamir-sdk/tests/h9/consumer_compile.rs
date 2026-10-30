//! Real rustc consumer checks, run only by CI's test harness.
use std::fs;
use std::process::Command;

fn compile(source: &str, canonical: bool) -> std::process::Output {
    let deps = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    // Multiple feature/profile builds can leave stale artifacts. Fail explicitly
    // rather than silently choosing an arbitrary SDK and testing the wrong code.
    let mut candidates: Vec<_> = fs::read_dir(&deps)
        .unwrap()
        .map(|entry| entry.expect("read SDK dependency directory").path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("libshamir_sdk-") && n.ends_with(".rlib"))
        })
        .collect();
    candidates.sort();
    assert_eq!(
        candidates.len(),
        1,
        "expected exactly one SDK rlib in {}; found: {candidates:?}",
        deps.display()
    );
    let sdk = &candidates[0];
    let root = std::env::temp_dir().join(format!(
        "shamir-sdk-consumer-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    fs::create_dir_all(&root).unwrap();
    let input = root.join("consumer.rs");
    fs::write(&input, source).unwrap();
    let name = if canonical { "shamir_sdk" } else { "renamed" };
    let output = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg("--edition=2021")
        .arg("--crate-name=consumer")
        .arg("--crate-type=lib")
        .arg("--emit=metadata")
        .arg("--out-dir")
        .arg(&root)
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("--extern")
        .arg(format!("{name}={}", sdk.display()))
        .arg(input)
        .output()
        .unwrap();
    fs::remove_dir_all(root).unwrap();
    output
}

#[test]
fn rv_sdk_c8189c06_canonical_and_renamed_only_consumers() {
    let signatures = [
        ("scalar", "p: sdk::Params", "sdk::Result<sdk::Value>", "let _ = p; let _ = sdk::Ctx::new(); Ok(sdk::Value::Null)"),
        ("procedure", "c: sdk::Ctx, p: sdk::Params", "sdk::Result<sdk::Value>", "let _ = p; let rows = c.db().table(\"users\").query(None)?; Ok(sdk::Value::List(rows))"),
        ("function", "_: sdk::Ctx, _: sdk::Batch, _: sdk::Params", "sdk::Result<sdk::Value>", "Ok(sdk::Value::Null)"),
        ("validator", "_: sdk::Value, _: Option<sdk::Value>, _: sdk::Ctx", "sdk::Validation", "sdk::Validation::accept()"),
    ];
    for (kind, args, ret, body) in signatures {
        for canonical in [true, false] {
            let dependency = if canonical { "shamir_sdk" } else { "renamed" };
            let attr = if canonical {
                String::new()
            } else {
                "(crate = sdk)".into()
            };
            let source = format!("use {dependency} as sdk; #[cfg(not(target_arch = \"wasm32\"))] fn shamir_alloc(_: i32) -> i32 {{ 0 }} #[cfg(not(target_arch = \"wasm32\"))] fn shamir_call(_: i32, _: i32) -> i64 {{ 0 }} #[sdk::{kind}{attr}] async fn entry({args}) -> {ret} {{ {body} }}");
            let output = compile(&source, canonical);
            assert!(
                output.status.success(),
                "{kind}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn rv_sdk_44a9cc7a_aliases_and_invalid_options() {
    let pass = "use renamed as sdk; type P = sdk::Params; #[sdk::scalar(crate = sdk)] async fn entry(_: P) -> sdk::Result<sdk::Value> { let _ = sdk::Ctx::new(); Ok(sdk::Value::Null) }";
    assert!(compile(pass, false).status.success());
    let fail = "use renamed as sdk; type C = sdk::Ctx; #[sdk::scalar(crate = sdk)] async fn entry(_: C) -> sdk::Result<sdk::Value> { Ok(sdk::Value::Null) }";
    let output = compile(fail, false);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mismatched types"));
    let direct = "use renamed as sdk; #[sdk::scalar(crate = sdk)] async fn entry(_: sdk::Ctx) -> sdk::Result<sdk::Value> { Ok(sdk::Value::Null) }";
    let output = compile(direct, false);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must not take Ctx"));
    let signatures = [
        (
            "scalar",
            "_: sdk::Params",
            "sdk::Result<sdk::Value>",
            "Ok(sdk::Value::Null)",
        ),
        (
            "procedure",
            "_: sdk::Ctx, _: sdk::Params",
            "sdk::Result<sdk::Value>",
            "Ok(sdk::Value::Null)",
        ),
        (
            "function",
            "_: sdk::Ctx, _: sdk::Batch, _: sdk::Params",
            "sdk::Result<sdk::Value>",
            "Ok(sdk::Value::Null)",
        ),
        (
            "validator",
            "_: sdk::Value, _: Option<sdk::Value>, _: sdk::Ctx",
            "sdk::Validation",
            "sdk::Validation::accept()",
        ),
    ];
    for (kind, params, ret, body) in signatures {
        for args in ["unknown = sdk", "crate = sdk, crate = sdk", "crate ="] {
            let source = format!("use renamed as sdk; #[sdk::{kind}({args})] async fn entry({params}) -> {ret} {{ {body} }}");
            assert!(!compile(&source, false).status.success(), "{kind} {args}");
        }
    }
}
