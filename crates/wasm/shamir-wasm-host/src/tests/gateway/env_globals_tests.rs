//! Tests for env-policy, env seeding, and atomic global-var RMW (slice 7).

use crate::{EnvPolicy, GlobalVars};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

// ── EnvPolicy union rule ──────────────────────────────────────────────

#[test]
fn env_policy_default_only_shamir() {
    let p = EnvPolicy::default();

    // Built-in SHAMIR_ prefix always included.
    assert!(p.includes("SHAMIR_X"));

    // Non-SHAMIR_ excluded by default.
    assert!(!p.includes("PATH"));

    // all=true → everything included.
    let p_all = EnvPolicy {
        all: true,
        ..EnvPolicy::default()
    };
    assert!(p_all.includes("PATH"));

    // Extra prefix.
    let p_prefix = EnvPolicy {
        prefixes: vec!["MY_".to_string()],
        ..EnvPolicy::default()
    };
    assert!(p_prefix.includes("MY_Y"));
    assert!(!p_prefix.includes("OTHER_Z"));

    // Exact name.
    let p_name = EnvPolicy {
        names: vec!["HOME".to_string()],
        ..EnvPolicy::default()
    };
    assert!(p_name.includes("HOME"));
    assert!(!p_name.includes("PATH"));

    // Glob mask.
    let p_mask = EnvPolicy {
        masks: vec!["AWS_*".to_string()],
        ..EnvPolicy::default()
    };
    assert!(p_mask.includes("AWS_KEY"));
    assert!(!p_mask.includes("BWS_KEY"));
}

// ── seed_env namespace and filtering ─────────────────────────────────

#[test]
fn seed_env_namespaces_and_filters() {
    let seeded_var = "SHAMIR_S7_SEED_TEST";
    let excluded_var = "NOTSEEDED_S7_X";
    std::env::set_var(seeded_var, "hello");
    std::env::set_var(excluded_var, "y");

    let g = GlobalVars::new();
    g.seed_env(&EnvPolicy::default());
    assert_eq!(
        g.get(&format!("env.{}", seeded_var)),
        Some(QueryValue::Str("hello".to_string()))
    );
    assert!(g.get(&format!("env.{}", excluded_var)).is_none());

    // With all=true, the excluded var is seeded.
    let g2 = GlobalVars::new();
    g2.seed_env(&EnvPolicy {
        all: true,
        ..EnvPolicy::default()
    });
    assert_eq!(
        g2.get(&format!("env.{}", excluded_var)),
        Some(QueryValue::Str("y".to_string()))
    );

    std::env::remove_var(seeded_var);
    std::env::remove_var(excluded_var);
}

// ── Atomic incr ──────────────────────────────────────────────────────

#[tokio::test]
async fn globals_incr_is_atomic() {
    let g = Arc::new(GlobalVars::new());
    let mut handles = Vec::new();
    for _ in 0..100 {
        let g = g.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            g.incr("s7_atomic_c", 1);
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(g.get("s7_atomic_c"), Some(QueryValue::Int(100)));
}

// ── Atomic update ────────────────────────────────────────────────────

#[test]
fn globals_update_replaces() {
    let g = GlobalVars::new();
    let v = g.update("s7_update_k", |_old| QueryValue::Int(5));
    assert_eq!(v, QueryValue::Int(5));

    let v2 = g.update("s7_update_k", |old| match old {
        Some(QueryValue::Int(n)) => QueryValue::Int(n + 1),
        _ => QueryValue::Int(0),
    });
    assert_eq!(v2, QueryValue::Int(6));
    assert_eq!(g.get("s7_update_k"), Some(QueryValue::Int(6)));
}
