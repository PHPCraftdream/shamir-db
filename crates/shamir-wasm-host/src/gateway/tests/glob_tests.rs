//! Tests for the shared `*`-only glob matcher (RV-WASM-HOST-1CF6A3BC /
//! RV-WASM-HOST-935F15D1).

use crate::gateway::env_policy::{glob_matches, EnvPolicy};
use crate::gateway::net_gateway::check_host_allowed;

#[test]
fn rv_wasm_host_1cf6a3bc_shared_matcher_used_by_both() {
    // After the consolidation there is exactly one matcher: the one in
    // `env_policy`, imported by `net_gateway`. This test drives BOTH
    // production consumers through the shared algorithm.
    let cases: &[(&str, &str, bool)] = &[
        ("*_KEY", "X_KEY", true),
        ("SHAMIR_*", "SHAMIR_DB_PATH", true),
        ("*.example.com", "api.example.com", true),
        ("*.example.com", "example.com", false),
        ("a*", "abc", true),
        ("a*", "bac", false),
        ("*a*", "xxaxx", true),
        ("exact", "exact", true),
        ("exact", "other", false),
    ];
    for (pattern, text, expected) in cases {
        assert_eq!(
            glob_matches(pattern, text),
            *expected,
            "pattern {pattern:?} vs text {text:?}"
        );
    }

    // ENV consumer: repeated-suffix pattern via EnvPolicy (fixes the
    // first-occurrence suffix bug; the old matcher rejected "X_KEY_KEY").
    let policy = EnvPolicy {
        masks: vec!["*_KEY".to_string()],
        ..Default::default()
    };
    assert!(policy.includes("X_KEY_KEY"));
    assert!(!policy.includes("KEY"));

    // NET consumer: same pattern through the egress guard. On the OLD
    // (pre-fix) matcher this returned `Err` because the first-occurrence
    // suffix bug rejected the match — restoring any stale net-local matcher
    // with the old algorithm fails this assertion.
    check_host_allowed("X_KEY_KEY", &["*_KEY".to_string()]).unwrap();

    // Multibyte path through the net consumer: no panic on a suffix split.
    assert!(check_host_allowed("é", &["*a".to_string()]).is_err());
    check_host_allowed("xé", &["*é".to_string()]).unwrap();
}

#[test]
fn rv_wasm_host_935f15d1_repeated_suffix_match() {
    // The final literal segment must anchor to the END of the text, so an
    // early occurrence of the suffix must not cause a false rejection.
    assert!(glob_matches("*_KEY", "X_KEY_KEY"));
    // ... and must not be satisfied by a text that lacks the suffix entirely.
    assert!(!glob_matches("*_KEY", "KEY"));
    // Start- and end-anchored multi-segment patterns.
    assert!(glob_matches("a*b*c", "aXbYc"));
    assert!(!glob_matches("a*b*c", "bXaYc"));
    // `*.example.com` must not match a lookalike suffix without the dot.
    assert!(!glob_matches("*.example.com", "evil-example.com"));
    // Start anchor preserved when the pattern doesn't start with `*`.
    assert!(!glob_matches("a*b", "xaXb"));
    // `*` alone matches everything.
    assert!(glob_matches("*", "anything"));
    assert!(glob_matches("*", ""));
}

#[test]
fn rv_wasm_host_935f15d1_utf8_suffix_boundary() {
    // The end-anchor suffix must not be sliced at an invalid UTF-8
    // boundary: `glob_matches("*a", "é")` PANICKED on the unfixed code
    // (start=1 lands inside the 2-byte `é`, so `text[1..]` is not a char
    // boundary). The fix returns `false` instead.
    assert!(!glob_matches("*a", "é"));
    // A multibyte suffix matched at a valid boundary still matches.
    assert!(glob_matches("*é", "xé"));
    assert!(glob_matches("*é", "é"));
}
