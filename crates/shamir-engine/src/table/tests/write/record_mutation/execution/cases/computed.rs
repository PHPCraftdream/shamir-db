use crate::table::tests::write::record_mutation::execution::shared::write_exec_tests::*;
use shamir_query_builder::val::*;
use shamir_query_builder::write::{self, doc};

// ============================================================================
// Computed write values ("установка знаний" via inline $fn)
// ============================================================================

#[tokio::test]
async fn test_insert_computed_field_lowercase() {
    let (table, repo) = setup_empty_table().await;

    // email_norm = strings/lower(email), evaluated at write time.
    let op = write::insert("users")
        .row(
            doc()
                .set("email", "Alice@Example.COM")
                .unwrap()
                .set("email_norm", func("strings/lower", [col("email")]))
                .unwrap(),
        )
        .build();

    let result = insert_via_tx(&repo, &table, &op, true).await.unwrap();
    assert_eq!(result.affected, 1);
    // The literal field is untouched; the computed field holds the result.
    assert_eq!(
        result.records[0].get_value_owned("email"),
        Some(shamir_types::types::value::QueryValue::Str(
            "Alice@Example.COM".to_string()
        ))
    );
    assert_eq!(
        result.records[0].get_value_owned("email_norm"),
        Some(shamir_types::types::value::QueryValue::Str(
            "alice@example.com".to_string()
        ))
    );
}

#[tokio::test]
async fn test_set_computed_field() {
    let (table, repo) = setup_empty_table().await;

    let op = write::upsert("users")
        .key(doc().set("email", "x@y.z").unwrap())
        .value(
            doc()
                .set("email", "X@Y.Z")
                .unwrap()
                .set("email_norm", func("strings/lower", [col("email")]))
                .unwrap(),
        )
        .build()
        .unwrap();

    let result = set_via_tx(&repo, &table, &op).await.unwrap();
    assert_eq!(
        result.records[0].get_value_owned("email_norm"),
        Some(shamir_types::types::value::QueryValue::Str(
            "x@y.z".to_string()
        ))
    );
}

#[tokio::test]
async fn test_insert_computed_unknown_function_fails_closed() {
    let (table, repo) = setup_empty_table().await;

    let op = write::insert("users")
        .row(doc().set("x", func("strings/does_not_exist", [])).unwrap())
        .build();

    // A broken computed value aborts the write rather than storing garbage.
    assert!(insert_via_tx(&repo, &table, &op, true).await.is_err());
}

#[tokio::test]
async fn test_insert_computed_bad_ref_fails_closed() {
    let (table, repo) = setup_empty_table().await;

    let op = write::insert("users")
        .row(
            doc()
                .set("y", func("strings/lower", [col("missing_field")]))
                .unwrap(),
        )
        .build();

    assert!(insert_via_tx(&repo, &table, &op, true).await.is_err());
}

// ============================================================================
// W3d-2: non-tx SET reroute parity (implicit-tx path)
// ============================================================================
