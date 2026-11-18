use crate::admin::types::schema_migration::{
    validate_migration_set, validate_query_value, validate_schema_rules, SchemaMigrationBody,
    SchemaMigrationError, SchemaMigrationFunctionDependencies, SchemaMigrationMetadata,
    SchemaMigrationOp,
};
use shamir_types::types::value::QueryValue;

fn metadata(id: &str, ordinal: u64, canonical_version: u16) -> SchemaMigrationMetadata {
    SchemaMigrationMetadata {
        id: id.to_owned(),
        ordinal,
        display_name: format!("migration {ordinal}"),
        canonical_version,
    }
}

fn body(id: &str, ordinal: u64) -> SchemaMigrationBody {
    SchemaMigrationBody {
        metadata: metadata(id, ordinal, 1),
        function_dependencies: Vec::new(),
        operations: Vec::new(),
    }
}

#[test]
fn metadata_rejects_empty_ids_zero_ordinals_and_unknown_canonical_versions() {
    assert_eq!(
        metadata(" ", 1, 1).validate(),
        Err(SchemaMigrationError::EmptyId)
    );
    assert_eq!(
        metadata("first", 0, 1).validate(),
        Err(SchemaMigrationError::OrdinalMustBePositive)
    );
    assert_eq!(
        metadata("first", 1, 2).validate(),
        Err(SchemaMigrationError::UnknownCanonicalVersion(2))
    );
}

#[test]
fn migration_set_requires_contiguous_input_order_unique_ids_and_ordinals() {
    assert!(validate_migration_set(&[body("one", 1), body("two", 2)]).is_ok());
    assert_eq!(
        validate_migration_set(&[body("one", 1), body("two", 3)]),
        Err(SchemaMigrationError::OrdinalGap {
            expected: 2,
            actual: 3
        })
    );
    assert_eq!(
        validate_migration_set(&[body("one", 1), body("one", 2)]),
        Err(SchemaMigrationError::DuplicateId("one".to_owned()))
    );
    assert_eq!(
        validate_migration_set(&[body("one", 1), body("two", 1)]),
        Err(SchemaMigrationError::DuplicateOrdinal(1))
    );
    assert_eq!(
        validate_migration_set(&[body("two", 2), body("one", 1)]),
        Err(SchemaMigrationError::OrdinalGap {
            expected: 1,
            actual: 2
        })
    );
}

#[test]
fn nested_schema_expressions_are_rejected_and_server_computed_flags_are_ignored() {
    let schema = vec![crate::admin::types::FieldRuleDto {
        path: vec!["x".to_owned()],
        r#type: "int".to_owned(),
        constraints: crate::admin::types::ConstraintsDto {
            default: Some(crate::filter::FilterValue::field_ref("other")),
            ..Default::default()
        },
        keyset_safe: false,
    }];
    assert!(validate_schema_rules(&schema).is_err());
    let schema = vec![crate::admin::types::FieldRuleDto {
        path: vec!["x".to_owned()],
        r#type: "int".to_owned(),
        constraints: Default::default(),
        keyset_safe: true,
    }];
    assert!(validate_schema_rules(&schema).is_ok());
}

#[test]
fn query_values_with_lossy_wire_variants_are_rejected() {
    let values = [
        QueryValue::Big("12345678901234567890".parse().unwrap()),
        QueryValue::Dec("1.25".parse().unwrap()),
        QueryValue::Set(shamir_collections::new_set()),
    ];
    for value in &values {
        assert!(matches!(
            validate_query_value(value, 0),
            Err(SchemaMigrationError::UnsafeOperation(_))
        ));
        let nested = QueryValue::List(vec![value.clone()]);
        assert!(matches!(
            validate_query_value(&nested, 0),
            Err(SchemaMigrationError::UnsafeOperation(_))
        ));
    }
}

#[test]
fn batch_admission_rejects_activity_listing() {
    // R4b-1: ListActivity is read-only server-state introspection, not a
    // schema-migration op; it is refused like every other non-DDL op.
    let rejected = crate::batch::BatchOp::ListActivity(crate::admin::ListActivityReq {
        scope: crate::admin::ActivityScope::All,
        kind: crate::admin::ActivityKind::All,
        limit: None,
        continuation: None,
    });
    assert!(matches!(
        SchemaMigrationOp::try_from(rejected),
        Err(SchemaMigrationError::UnsupportedOperation(
            "activity listing"
        ))
    ));
}

#[test]
fn batch_admission_rejects_arbitrary_function_invocation() {
    let rejected = crate::batch::BatchOp::Call(crate::CallOp::new(
        "invoke",
        Vec::<crate::filter::FilterValue>::new(),
    ));
    assert!(matches!(
        SchemaMigrationOp::try_from(rejected),
        Err(SchemaMigrationError::UnsupportedOperation(
            "arbitrary function invocation"
        ))
    ));
}

#[test]
fn dependency_names_must_be_unique_and_nonempty() {
    let mut migration = body("one", 1);
    migration
        .function_dependencies
        .push(SchemaMigrationFunctionDependencies {
            function: "f".to_owned(),
            depends_on: vec!["g".to_owned(), "g".to_owned()],
        });
    assert!(migration.validate().is_err());
}

#[test]
fn query_value_literals_are_bounded() {
    let value = QueryValue::List(vec![QueryValue::Str("x".to_owned())]);
    assert!(validate_query_value(&value, 0).is_ok());
}
