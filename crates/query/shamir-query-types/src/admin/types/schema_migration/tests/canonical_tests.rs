use crate::admin::types::schema_migration::canonical::{
    canonical_body, canonical_body_checksum, canonical_query_value, canonical_value,
};
use crate::admin::types::schema_migration::{
    SchemaMigrationBody, SchemaMigrationFunctionDependencies, SchemaMigrationMetadata,
    SchemaMigrationOp,
};
use shamir_collections::new_map;
use shamir_types::types::value::QueryValue;

fn empty_body() -> SchemaMigrationBody {
    SchemaMigrationBody {
        metadata: SchemaMigrationMetadata {
            id: "seed".to_owned(),
            ordinal: 1,
            display_name: "Initial schema".to_owned(),
            canonical_version: 1,
        },
        function_dependencies: Vec::new(),
        operations: Vec::new(),
    }
}

#[cfg(feature = "crypto")]
#[test]
fn function_dependency_order_does_not_affect_checksum() {
    let mut left = empty_body();
    left.function_dependencies = vec![
        SchemaMigrationFunctionDependencies {
            function: "f".to_owned(),
            depends_on: vec!["b".to_owned(), "a".to_owned()],
        },
        SchemaMigrationFunctionDependencies {
            function: "g".to_owned(),
            depends_on: vec!["z".to_owned()],
        },
    ];
    let mut right = left.clone();
    right.function_dependencies.reverse();
    right.function_dependencies[1].depends_on.reverse();
    assert_eq!(
        canonical_body_checksum(&left).unwrap(),
        canonical_body_checksum(&right).unwrap()
    );
}

#[cfg(feature = "crypto")]
#[test]
fn canonical_body_uses_metadata_and_ordered_operation_sequence() {
    let first = empty_body();
    let mut different_name = empty_body();
    different_name.metadata.id = "changed".to_owned();
    assert_ne!(
        canonical_body(&first).unwrap(),
        canonical_body(&different_name).unwrap()
    );
    assert_ne!(
        canonical_body(&first).unwrap(),
        canonical_body_checksum(&first).unwrap()
    );
    assert_eq!(canonical_body_checksum(&first).unwrap().len(), 32);
}

#[cfg(feature = "crypto")]
#[test]
fn canonical_encoding_preserves_source_bytes_and_operation_order() {
    let mut first = empty_body();
    first
        .operations
        .push(SchemaMigrationOp::CreateFunction(Box::new(
            crate::admin::types::CreateFunctionOp {
                create_function: "f".to_owned(),
                source: Some("source\n".to_owned()),
                wasm: None,
                replace: false,
                visibility: None,
                security: None,
                secret_grants: Vec::new(),
                net_grants: Vec::new(),
                lang: None,
                optimize: None,
                hmac: None,
            },
        )));
    let mut line_ending = first.clone();
    let SchemaMigrationOp::CreateFunction(function) = &mut line_ending.operations[0] else {
        unreachable!()
    };
    function.source = Some("source\r\n".to_owned());
    assert_ne!(
        canonical_body_checksum(&first).unwrap(),
        canonical_body_checksum(&line_ending).unwrap()
    );
}

#[cfg(feature = "crypto")]
#[test]
fn canonical_maps_ignore_insertion_order_while_lists_keep_effect_order() {
    let mut forward = new_map();
    forward.insert("a".to_owned(), QueryValue::Int(1));
    forward.insert("b".to_owned(), QueryValue::Int(2));
    let mut reverse = new_map();
    reverse.insert("b".to_owned(), QueryValue::Int(2));
    reverse.insert("a".to_owned(), QueryValue::Int(1));
    assert_eq!(
        canonical_value(&QueryValue::Map(forward)).unwrap(),
        canonical_value(&QueryValue::Map(reverse)).unwrap()
    );

    let left = QueryValue::List(vec![QueryValue::Int(1), QueryValue::Int(2)]);
    let right = QueryValue::List(vec![QueryValue::Int(2), QueryValue::Int(1)]);
    assert_ne!(
        canonical_value(&left).unwrap(),
        canonical_value(&right).unwrap()
    );
}

#[cfg(feature = "crypto")]
#[test]
fn migration_body_msgpack_round_trips_nested_admitted_values_and_checksum() {
    // Serde round-trip is the wire-format subject; query construction remains typed.
    let mut body = empty_body();
    let mut nested_map = new_map();
    nested_map.insert("label".to_owned(), QueryValue::Str("é".to_owned()));
    let nested_values = vec![QueryValue::List(vec![
        QueryValue::Null,
        QueryValue::Bool(true),
        QueryValue::Int(17),
        QueryValue::F64(1.5),
        QueryValue::Bin(vec![0, 255, 1]),
        QueryValue::Map(nested_map),
    ])];
    // Nested admitted values survive the wire through the schema constraint `one_of`;
    // `functional_args` are refused by admission and must not carry round-trip coverage.
    body.operations
        .push(SchemaMigrationOp::AddSchemaRule(Box::new(
            crate::admin::types::AddSchemaRuleOp {
                add_schema_rule: "items".to_owned(),
                repo: "main".to_owned(),
                rule: crate::admin::types::FieldRuleDto {
                    path: vec!["label".to_owned()],
                    r#type: "any".to_owned(),
                    constraints: crate::admin::types::ConstraintsDto {
                        one_of: Some(nested_values),
                        ..Default::default()
                    },
                    keyset_safe: false,
                },
            },
        )));
    let checksum = canonical_body_checksum(&body).unwrap();
    let encoded = rmp_serde::to_vec_named(&body).unwrap();
    let decoded: SchemaMigrationBody = rmp_serde::from_slice(&encoded).unwrap();
    assert_eq!(decoded, body);
    assert_eq!(canonical_body_checksum(&decoded).unwrap(), checksum);
}

#[cfg(feature = "crypto")]
#[test]
fn server_computed_schema_flags_do_not_change_canonical_checksum() {
    let mut safe = empty_body();
    let mut rule = crate::admin::types::FieldRuleDto {
        path: vec!["x".to_owned()],
        r#type: "int".to_owned(),
        constraints: Default::default(),
        keyset_safe: false,
    };
    safe.operations
        .push(SchemaMigrationOp::CreateTable(Box::new(
            crate::admin::types::CreateTableOp {
                create_table: "items".to_owned(),
                repo: "main".to_owned(),
                if_not_exists: false,
                retention: None,
                schema: Some(vec![rule.clone()]),
            },
        )));
    assert!(safe.validate().is_ok());
    let checksum = canonical_body_checksum(&safe).unwrap();
    rule.keyset_safe = true;
    let mut computed = safe.clone();
    let SchemaMigrationOp::CreateTable(operation) = &mut computed.operations[0] else {
        unreachable!()
    };
    operation.schema = Some(vec![rule]);
    assert!(computed.validate().is_ok());
    assert_eq!(canonical_body_checksum(&computed).unwrap(), checksum);
}

#[cfg(feature = "crypto")]
#[test]
fn canonical_body_rejects_unknown_version() {
    let mut body = empty_body();
    body.metadata.canonical_version = 2;
    assert!(matches!(
        canonical_body(&body),
        Err(
            crate::admin::types::schema_migration::SchemaMigrationCanonicalError::UnknownVersion(2)
        )
    ));
}

#[test]
fn query_value_literals_are_canonically_encodable() {
    let value = QueryValue::List(vec![QueryValue::Str("x".to_owned())]);
    assert!(canonical_query_value(&value).is_ok());
}
