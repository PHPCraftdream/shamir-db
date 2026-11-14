//! Self-tests for the `layers` gate (CI-only): each test materialises
//! a fixture tree into a throwaway git repo and runs the built binary
//! against it, asserting the exact finding code or a clean exit.

use crate::harness::{assert_clean, assert_finding, TempRepo};

/// Reads the fixture's baseline file inside the temp repo.
fn baseline_of(repo: &TempRepo) -> String {
    std::fs::read_to_string(repo.root.join(".rust-guard/layers_baseline.toml"))
        .expect("read baseline")
}

#[test]
fn clean_crate_passes() {
    // Downward references only (plus upward ones in #[cfg(test)] code,
    // which never fails the gate): clean run.
    let repo = TempRepo::with_fixture("layers/layers_ok");
    assert_clean(&repo.run_guard(&["layers"]));
}

#[test]
fn plain_use_upward_is_backward() {
    let repo = TempRepo::with_fixture("layers/layers_use_backward");
    let output = repo.run_guard(&["layers"]);
    assert_finding(&output, "LAYER_BACKWARD");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("allowed direction is b -> a"),
        "expected direction hint: {stderr}"
    );
}

#[test]
fn grouped_use_is_one_finding_per_file() {
    // One grouped `use` with two items is ONE backward reference:
    // exactly one finding line for the file.
    let repo = TempRepo::with_fixture("layers/layers_grouped_use");
    let output = repo.run_guard(&["layers"]);
    assert_finding(&output, "LAYER_BACKWARD");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("LAYER_BACKWARD").count(),
        1,
        "expected one finding: {stderr}"
    );
}

#[test]
fn crate_root_reexport_resolves_to_owner() {
    // `use crate::inner::Item` resolves through the crate-root
    // re-export into layer b's `b::inner` module.
    let repo = TempRepo::with_fixture("layers/layers_reexport_root");
    assert_finding(&repo.run_guard(&["layers"]), "LAYER_BACKWARD");
}

#[test]
fn code_path_without_use_is_backward() {
    let repo = TempRepo::with_fixture("layers/layers_code_path");
    assert_finding(&repo.run_guard(&["layers"]), "LAYER_BACKWARD");
}

#[test]
fn super_chain_leaving_the_module_is_backward() {
    // `super::super::b::Engine` from a layer-a submodule leaves the
    // module and reaches up into layer b.
    let repo = TempRepo::with_fixture("layers/layers_super_chain");
    assert_finding(&repo.run_guard(&["layers"]), "LAYER_BACKWARD");
}

#[test]
fn impl_self_type_of_other_module_is_backward() {
    // One finding for the file: the `use` and the `impl` share it.
    let repo = TempRepo::with_fixture("layers/layers_impl");
    assert_finding(&repo.run_guard(&["layers"]), "LAYER_BACKWARD");
}

#[test]
fn crate_path_inside_macro_tokens_is_backward() {
    // The upward reference only exists in `macro_rules!` tokens
    // (`take!(crate::b::Thing)`); token scanning must still catch it.
    let repo = TempRepo::with_fixture("layers/layers_macro_path");
    assert_finding(&repo.run_guard(&["layers"]), "LAYER_BACKWARD");
}

#[test]
fn inline_module_item_is_an_edge() {
    // `crate::b::inner::go()` into an INLINE module (`mod inner {}`,
    // no file of its own) is still a backward edge.
    let repo = TempRepo::with_fixture("layers/layers_inline_mod");
    assert_finding(&repo.run_guard(&["layers"]), "LAYER_BACKWARD");
}

#[test]
fn baseline_entry_at_zero_is_stale() {
    // No backward references exist, but the baseline claims one.
    let repo = TempRepo::with_fixture("layers/layers_baseline_stale");
    assert_finding(&repo.run_guard(&["layers"]), "STALE_BASELINE");
}

#[test]
fn update_prunes_stale_entry() {
    let repo = TempRepo::with_fixture("layers/layers_baseline_stale");
    assert_clean(&repo.run_guard(&["layers", "--update-baseline"]));
    assert!(
        !baseline_of(&repo).contains("a -> b"),
        "stale entry must be pruned: {}",
        baseline_of(&repo)
    );
}

#[test]
fn grown_pair_fails() {
    // Two layer-a files reach up; the baseline allows one.
    let repo = TempRepo::with_fixture("layers/layers_baseline_grow");
    let output = repo.run_guard(&["layers"]);
    assert_finding(&output, "LAYER_BACKWARD");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("LAYER_BACKWARD").count(),
        2,
        "expected two findings: {stderr}"
    );
}

#[test]
fn update_refuses_growth() {
    let repo = TempRepo::with_fixture("layers/layers_baseline_grow");
    let output = repo.run_guard(&["layers", "--update-baseline"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "expected exit != 0:\n{stderr}");
    assert!(stderr.contains("rust-guard: error:"), "{stderr}");
    assert!(stderr.contains("--update-baseline refused"), "{stderr}");
    assert!(
        baseline_of(&repo).contains("\"a -> b\" = 1"),
        "baseline must stay unchanged: {}",
        baseline_of(&repo)
    );
}

#[test]
fn reexport_shadow_is_not_an_edge() {
    // `Alpha` re-exports an EXTERNAL item: not an edge into b. Only the
    // genuinely local `Engine` produces one LAYER_BACKWARD.
    let repo = TempRepo::with_fixture("layers/layers_reexport_shadow");
    let output = repo.run_guard(&["layers"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "expected exit != 0:\n{stderr}");
    assert_eq!(
        stderr.matches("LAYER_BACKWARD").count(),
        1,
        "expected one finding: {stderr}"
    );
    assert!(stderr.contains("a -> b"), "{stderr}");
}

#[test]
fn glob_shadow_is_not_an_edge() {
    // A glob re-exporting an EXTERNAL module shadows `Ghost`; only the
    // local `Engine` produces one LAYER_BACKWARD.
    let repo = TempRepo::with_fixture("layers/layers_glob_shadow");
    let output = repo.run_guard(&["layers"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "expected exit != 0:\n{stderr}");
    assert_eq!(
        stderr.matches("LAYER_BACKWARD").count(),
        1,
        "expected one finding: {stderr}"
    );
    assert!(stderr.contains("a -> b"), "{stderr}");
}

#[test]
fn layer_name_mapping_names_pairs_by_layer() {
    // Layer `inst` maps to module `b`; the pair key uses the LAYER
    // name, and --update-baseline rewrites the same count.
    let repo = TempRepo::with_fixture("layers/layers_module_alias");
    assert_clean(&repo.run_guard(&["layers"]));
    assert_clean(&repo.run_guard(&["layers", "--update-baseline"]));
    assert!(
        baseline_of(&repo).contains("\"a -> inst\" = 1"),
        "{}",
        baseline_of(&repo)
    );
}

#[test]
fn lower_pair_passes() {
    // One file reaches up; the baseline allows two (strictly lower is
    // fine on a normal run).
    let repo = TempRepo::with_fixture("layers/layers_update_lower");
    assert_clean(&repo.run_guard(&["layers"]));
}

#[test]
fn update_lowers_entry() {
    let repo = TempRepo::with_fixture("layers/layers_update_lower");
    let output = repo.run_guard(&["layers", "--update-baseline"]);
    assert!(output.status.success(), "expected exit 0");
    let baseline = baseline_of(&repo);
    assert!(baseline.contains("\"a -> b\" = 1"), "{baseline}");
    assert!(!baseline.contains("= 2"), "{baseline}");
}
