//! Round 7 (G1): the ONE in-doubt decision that validator replace, rename and
//! drop all consume, tested directly on the evidence set.
//!
//! Before round 7 the refusal lived in the drop caller only: a doubtful table
//! put its database into the reservation set, REPLACE and RENAME consumed just
//! that set, and a vacant fence slot reserves successfully — so in the double
//! fault state (the binding flush fails AND the compensation cannot be made
//! durable) a free database let a foreign REPLACE/RENAME swap the artifact the
//! doubtful table still invokes. These tests pin the decision that closes it.

use super::super::binding_scan::BindingHolders;
use crate::DbError;

fn holders(dbs: &[&str], tables: &[&str], in_doubt: &[&str]) -> BindingHolders {
    BindingHolders::for_test(
        dbs.iter().map(|name| (*name).to_string()).collect(),
        tables.iter().map(|name| (*name).to_string()).collect(),
        in_doubt.iter().map(|name| (*name).to_string()).collect(),
    )
}

fn refusal(holders: &BindingHolders) -> DbError {
    holders
        .refuse_in_doubt()
        .expect_err("in-doubt evidence must refuse")
}

#[test]
fn no_holders_and_no_doubt_admits() {
    let evidence = holders(&[], &[], &[]);
    assert!(evidence.is_empty());
    assert!(!evidence.has_in_doubt());
    evidence
        .refuse_in_doubt()
        .expect("an empty evidence set must admit the operation");
}

#[test]
fn named_holders_without_doubt_admit() {
    let evidence = holders(&["app"], &["app/main/users"], &[]);
    assert!(!evidence.is_empty());
    assert!(!evidence.has_in_doubt());
    evidence
        .refuse_in_doubt()
        .expect("named holders alone are a fence reservation, not an in-doubt refusal");
}

#[test]
fn doubt_alone_refuses_without_disclosing_a_path() {
    let evidence = holders(&["app"], &[], &["app"]);
    assert!(evidence.has_in_doubt());
    assert!(
        !evidence.is_empty(),
        "an in-doubt table alone still makes the evidence non-empty"
    );
    let error = refusal(&evidence);
    assert!(matches!(error, DbError::Conflict(_)), "{error}");
    let message = error.to_string();
    assert!(message.contains("in doubt"), "{message}");
    assert!(
        !message.contains("app"),
        "the refusal must not disclose the database or table: {message}"
    );
}

#[test]
fn mixed_evidence_refuses_with_the_same_text_as_doubt_alone() {
    let doubt_only = refusal(&holders(&["app"], &[], &["app"])).to_string();
    let mixed = refusal(&holders(&["app", "other"], &["app/main/users"], &["other"]));
    assert!(matches!(mixed, DbError::Conflict(_)), "{mixed}");
    assert_eq!(
        mixed.to_string(),
        doubt_only,
        "one refusal text for every caller and every evidence mix"
    );
    assert!(
        !mixed.to_string().contains("users"),
        "named holders must not leak through the in-doubt text: {mixed}"
    );
}
