use std::fs;
use std::path::Path;
use std::process::Command;

mod harness;
use harness::{assert_clean, assert_finding, TempRepo};

const PATH: &str = "crates/fix/src/lib.rs";
const PIN_PREFIX: &str = "path = \"crates/fix/src/lib.rs\"\nitem = \"f\"\nowner = \"test-owner\"\nreason = \"regression contract\"\ncalls = ";

fn run(source: &str, calls: &str, extra: &str) -> (TempRepo, std::process::Output) {
    let repo = TempRepo::with_fixture("orderings/base");
    let source_path = repo.root.join(PATH);
    fs::create_dir_all(source_path.parent().unwrap()).unwrap();
    fs::write(&source_path, source).unwrap();
    let pins = repo.root.join("pins.toml");
    fs::write(&pins, format!("[[pin]]\n{PIN_PREFIX}{calls}\n{extra}")).unwrap();
    let output = repo.run_guard(&["orderings", "--pins", pins.to_str().unwrap()]);
    (repo, output)
}
fn assert_finding_exact(out: &std::process::Output, expected: &[&str]) {
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "expected failure: {text}");
    assert!(
        !text.contains("rust-guard: error"),
        "unexpected infrastructure failure: {text}"
    );
    let codes = [
        "MISSING_ITEM",
        "WEAKENED_ORDERING",
        "NON_LITERAL_ORDERING",
        "UNPINNED_CALL",
        "MISSING_FENCE",
        "STALE_PIN",
    ];
    let mut actual = Vec::new();
    let mut finding_count = None;
    for line in text.lines() {
        if let Some(code) = codes
            .iter()
            .find(|code| line.starts_with(*code) || line.contains(&format!(": {}:", code)))
        {
            actual.push(*code);
        }
        if let Some(count) = line
            .strip_prefix("rust-guard: ")
            .and_then(|s| s.strip_suffix(" finding(s)"))
            .and_then(|s| s.parse::<usize>().ok())
        {
            finding_count = Some(count);
        }
    }
    assert_eq!(
        finding_count,
        Some(expected.len()),
        "finding total mismatch: {text}"
    );
    assert_eq!(
        actual, expected,
        "unexpected finding lines or codes: {text}"
    );
}
fn assert_clean_or_errorless(out: &std::process::Output) {
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "expected success: {text}");
    assert!(text.contains("0 finding"), "expected zero findings: {text}");
}
fn clean(source: &str) {
    let (_repo, out) = run(source, "[{ method = \"fetch_add\", min = \"SeqCst\" }]", "");
    assert_clean(&out);
}
fn fails(source: &str, calls: &str, extra: &str, code: &str) {
    let (_repo, out) = run(source, calls, extra);
    assert_finding(&out, code);
    assert_finding_exact(&out, &[code]);
}
fn run_cfg(source: &str, branch: &str) -> std::process::Output {
    let repo = TempRepo::with_fixture("orderings/base");
    fs::write(repo.root.join(PATH), source).unwrap();
    let pins = repo.root.join("pins.toml");
    fs::write(&pins, format!("[[pin]]\n{PIN_PREFIX}[{{ method=\"fetch_add\", min=\"SeqCst\" }}]\nbranch=\"{branch}\"\n")).unwrap();
    repo.run_guard(&["orderings", "--pins", pins.to_str().unwrap()])
}

#[test]
fn clean_and_mutation_cases() {
    clean("use std::sync::atomic::Ordering; fn f() { a.fetch_add(1, Ordering::SeqCst); }");
    fails(
        "use std::sync::atomic::Ordering; fn f() { a.fetch_add(1, Ordering::Relaxed); }",
        "[{ method = \"fetch_add\", min = \"SeqCst\" }]",
        "",
        "WEAKENED_ORDERING",
    );
    fails("fn renamed() {}", "[]", "", "MISSING_ITEM");
    fails(
        "const O: Ordering = Ordering::SeqCst; fn f() { a.fetch_add(1, O); }",
        "[{ method = \"fetch_add\", min = \"SeqCst\" }]",
        "",
        "NON_LITERAL_ORDERING",
    );
    fails(
        "fn f() { a.fetch_add(1, Ordering::SeqCst); a.swap(0, Ordering::SeqCst); }",
        "[{ method = \"fetch_add\", min = \"SeqCst\" }]",
        "",
        "UNPINNED_CALL",
    );
    fails(
        "fn f() { a.fetch_add(1, Ordering::SeqCst); }",
        "[{ method = \"fetch_add\", min = \"SeqCst\" }]",
        "require_fence = \"SeqCst\"",
        "MISSING_FENCE",
    );
    fails("fn f() { a.fetch_add(1, Ordering::SeqCst); }", "[{ method = \"fetch_add\", min = \"SeqCst\" }, { method = \"fetch_sub\", min = \"SeqCst\" }]", "", "STALE_PIN");
}

#[test]
fn call_ordering_weakening_and_stale_pin_are_exact() {
    fails(
        "fn f() { a.fetch_add(1, Ordering::Relaxed); }",
        "[{ method = \"fetch_add\", min = \"SeqCst\" }]",
        "",
        "WEAKENED_ORDERING",
    );
    fails("fn f() { a.fetch_add(1, Ordering::SeqCst); }", "[{ method = \"fetch_add\", min = \"SeqCst\" }, { method = \"fetch_sub\", min = \"SeqCst\" }]", "", "STALE_PIN");
}
#[test]
fn cas_checks_both_success_and_failure_orderings() {
    fails(
        "fn f() { a.compare_exchange(0, 1, Ordering::Relaxed, Ordering::Acquire); }",
        "[{ method = \"compare_exchange\", min = \"SeqCst\" }]",
        "",
        "WEAKENED_ORDERING",
    );
    fails(
        "fn f() { a.compare_exchange(0, 1, Ordering::SeqCst, Ordering::Relaxed); }",
        "[{ method = \"compare_exchange\", min = \"SeqCst\" }]",
        "",
        "WEAKENED_ORDERING",
    );
}

#[test]
fn production_and_loom_definition_selection() {
    let repo = TempRepo::with_fixture("orderings/base");
    fs::write(repo.root.join(PATH), "#[cfg(not(loom))] fn f() { a.fetch_add(1, Ordering::SeqCst); } #[cfg(loom)] fn f() { a.fetch_add(1, Ordering::Relaxed); }").unwrap();
    let pins = repo.root.join("pins.toml");
    fs::write(&pins, format!("[[pin]]\n{PIN_PREFIX}[{{ method=\"fetch_add\", min=\"SeqCst\" }}]\n\n[[pin]]\n{PIN_PREFIX}[{{ method=\"fetch_add\", min=\"Relaxed\" }}]\nbranch=\"loom\"\n")).unwrap();
    let out = repo.run_guard(&["orderings", "--pins", pins.to_str().unwrap()]);
    assert_clean(&out);
}

#[test]
fn cfg_selection_tests() {
    assert_clean_or_errorless(&run_cfg(
        "#[cfg(all(feature = \"x\", loom))] fn f() { a.fetch_add(1, Ordering::SeqCst); }",
        "loom",
    ));
    assert_clean_or_errorless(&run_cfg(
        "#[cfg(all(feature = \"x\", not(loom)))] fn f() { a.fetch_add(1, Ordering::SeqCst); }",
        "production",
    ));
}

#[test]
fn nested_statement_cfg_selects_only_matching_branch() {
    let source = "fn f() { #[cfg(all(feature = \"x\", loom))] { a.fetch_add(1, Ordering::Relaxed); } #[cfg(all(feature = \"x\", not(loom)))] { a.fetch_add(1, Ordering::SeqCst); } } #[cfg(loom)] fn g() { #[cfg(all(feature = \"x\", loom))] { a.fetch_add(1, Ordering::Relaxed); } #[cfg(all(feature = \"x\", not(loom)))] { a.fetch_add(1, Ordering::SeqCst); } }";
    let repo = TempRepo::with_fixture("orderings/base");
    fs::write(repo.root.join(PATH), source).unwrap();
    let pins = repo.root.join("pins.toml");
    fs::write(&pins, format!("[[pin]]\n{PIN_PREFIX}[{{ method=\"fetch_add\", min=\"SeqCst\" }}]\n\n[[pin]]\npath = \"crates/fix/src/lib.rs\"\nitem = \"g\"\nbranch = \"loom\"\nowner = \"test-owner\"\nreason = \"regression contract\"\ncalls = [{{ method=\"fetch_add\", min=\"SeqCst\" }}]\n")).unwrap();
    let output = repo.run_guard(&["orderings", "--pins", pins.to_str().unwrap()]);
    assert_finding_exact(&output, &["WEAKENED_ORDERING"]);
}

#[test]
fn macro_token_arguments_are_scanned() {
    fails(
        "fn f() { macro_args!(a.fetch_add(1, Ordering::Relaxed),); }",
        "[{ method = \"fetch_add\", min = \"SeqCst\" }]",
        "",
        "WEAKENED_ORDERING",
    );
}

#[test]
fn load_and_fetch_update_positions_are_checked() {
    fails(
        "fn f() { a.load(Ordering::Relaxed); }",
        "[{ method = \"load\", min = \"SeqCst\" }]",
        "",
        "WEAKENED_ORDERING",
    );
    fails(
        "fn f() { a.store(0, Ordering::Relaxed); }",
        "[{ method = \"store\", min = \"SeqCst\" }]",
        "",
        "WEAKENED_ORDERING",
    );
    fails(
        "fn f() { a.swap(0, Ordering::Relaxed); }",
        "[{ method = \"swap\", min = \"SeqCst\" }]",
        "",
        "WEAKENED_ORDERING",
    );
    fails(
        "fn f() { a.fetch_update(Ordering::Relaxed, Ordering::Acquire, |_| None); }",
        "[{ method = \"fetch_update\", min = \"SeqCst\" }]",
        "",
        "WEAKENED_ORDERING",
    );
}
#[test]
fn fences_are_detected() {
    let (_repo, out) = run(
        "fn f() { atomic::fence(Ordering::SeqCst); a.fetch_add(1, Ordering::SeqCst); }",
        "[{ method = \"fetch_add\", min = \"SeqCst\" }]",
        "require_fence = \"SeqCst\"",
    );
    assert_clean(&out);
}
#[test]
fn ufcs_drain_fixture_detects_weakened_loop_with_diagnostic_loads() {
    let repo = TempRepo::with_fixture("orderings/base");
    let source_path = repo.root.join("crates/fix/src/drain.rs");
    let source = fs::read_to_string(&source_path).unwrap();
    let pins = repo.root.join("pins.toml");
    fs::write(&pins, "[[pin]]\npath = \"crates/fix/src/drain.rs\"\nitem = \"WriterDrainBarrier::drain\"\nowner = \"test-owner\"\nreason = \"drain ordering\"\ncalls = [{ method = \"load\", min = \"SeqCst\" }]\n").unwrap();
    let args = ["orderings", "--pins", pins.to_str().unwrap()];
    assert_clean(&repo.run_guard(&args));

    let original = "AtomicUsize::load(&self.active, Ordering::SeqCst)";
    assert_eq!(source.matches(original).count(), 1);
    fs::write(
        &source_path,
        source.replace(
            original,
            "AtomicUsize::load(&self.active, Ordering::Relaxed)",
        ),
    )
    .unwrap();
    assert_finding_exact(&repo.run_guard(&args), &["WEAKENED_ORDERING"]);
}

#[test]
fn ufcs_paths_and_ordering_arguments_are_checked() {
    let load = "[{ method = \"load\", min = \"SeqCst\" }]";
    for ty in [
        "AtomicUsize",
        "atomic::AtomicUsize",
        "std::sync::atomic::AtomicUsize",
    ] {
        let source = format!("fn f() {{ {ty}::load(&mut self.active, Ordering::Relaxed); }}");
        fails(&source, load, "", "WEAKENED_ORDERING");
        let source = format!("fn f() {{ {ty}::load(&self.active, Ordering::SeqCst); }}");
        let (_repo, out) = run(&source, load, "");
        assert_clean(&out);
    }
    fails(
        "fn f() { AtomicBool::load(&a, order); }",
        load,
        "",
        "NON_LITERAL_ORDERING",
    );
    fails("fn f() { AtomicUsize::load(&a, Ordering::SeqCst); AtomicUsize::swap(&a, 0, Ordering::SeqCst); }", load, "", "UNPINNED_CALL");
    let (_repo, out) = run("fn f() { Other::load(&a, Ordering::Relaxed); }", "[]", "");
    assert_clean(&out);
}

#[test]
fn ufcs_qself_and_same_file_aliases_are_checked() {
    let load = "[{ method = \"load\", min = \"SeqCst\" }]";
    fails(
        "fn f() { <AtomicUsize>::load(&a, Ordering::Relaxed); }",
        load,
        "",
        "WEAKENED_ORDERING",
    );
    let (_repo, out) = run(
        "fn f() { <AtomicUsize>::load(&a, Ordering::SeqCst); }",
        load,
        "",
    );
    assert_clean(&out);
    for import in [
        "use std::sync::atomic::AtomicUsize as Counter;",
        "use std::{sync::{atomic::{AtomicUsize as Counter, Ordering}}};",
    ] {
        let source = format!("{import} fn f() {{ Counter::load(&a, Ordering::Relaxed); }}");
        fails(&source, load, "", "WEAKENED_ORDERING");
        let source = format!("{import} fn f() {{ <Counter>::load(&a, Ordering::SeqCst); }}");
        let (_repo, out) = run(&source, load, "");
        assert_clean(&out);
    }
    let (_repo, out) = run(
        "use other::Thing as Counter; fn f() { Counter::load(&a, Ordering::Relaxed); }",
        "[]",
        "",
    );
    assert_clean(&out);
}

#[test]
fn ufcs_cas_and_fetch_update_failure_orderings_are_checked() {
    for method in ["compare_exchange", "compare_exchange_weak"] {
        let pins = format!("[{{ method = \"{method}\", min = \"SeqCst\" }}]");
        for (success, failure) in [
            ("Relaxed", "Acquire"),
            ("SeqCst", "Relaxed"),
            ("SeqCst", "Release"),
        ] {
            let source = format!("fn f() {{ AtomicUsize::{method}(&a, 0, 1, Ordering::{success}, Ordering::{failure}); }}");
            fails(&source, &pins, "", "WEAKENED_ORDERING");
        }
        let source = format!(
            "fn f() {{ AtomicUsize::{method}(&a, 0, 1, Ordering::SeqCst, Ordering::Acquire); }}"
        );
        let (_repo, out) = run(&source, &pins, "");
        assert_clean(&out);
    }
    fails("fn f() { atomic::AtomicUsize::fetch_update(&a, Ordering::SeqCst, Ordering::Relaxed, |_| None); }", "[{ method = \"fetch_update\", min = \"SeqCst\" }]", "", "WEAKENED_ORDERING");
}

#[test]
fn real_pin_file_passes_real_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let pins = root.join(".rust-guard/atomic_orderings.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_rust-guard"))
        .arg("--repo")
        .arg(&root)
        .arg("orderings")
        .arg("--pins")
        .arg(pins)
        .output()
        .unwrap();
    assert_clean(&output);
}

#[test]
fn negated_composite_cfg_keeps_reachable_loom_statement() {
    // `not(all(loom, feature))` is still compilable under loom when `feature` is off.
    let out = run_cfg(
        "#[cfg(loom)] fn f() { #[cfg(not(all(loom, feature = \"x\")))] { a.fetch_add(1, Ordering::Relaxed); } }",
        "loom",
    );
    assert_finding_exact(&out, &["WEAKENED_ORDERING"]);
}

#[test]
fn fetch_update_failure_ordering_is_checked() {
    fails(
        "fn f() { a.fetch_update(Ordering::SeqCst, Ordering::Relaxed, |_| None); }",
        "[{ method = \"fetch_update\", min = \"SeqCst\" }]",
        "",
        "WEAKENED_ORDERING",
    );
}
