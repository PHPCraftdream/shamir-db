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

fn create_index(
    index_type: Option<&str>,
    functional_op: Option<&str>,
    functional_args: Option<Vec<QueryValue>>,
) -> SchemaMigrationOp {
    SchemaMigrationOp::CreateIndex(Box::new(crate::admin::types::CreateIndexOp {
        create_index: "items_by_label".to_owned(),
        table: "items".to_owned(),
        fields: vec![vec!["label".to_owned()]],
        unique: false,
        sorted: false,
        repo: "main".to_owned(),
        index_type: index_type.map(str::to_owned),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: functional_op.map(str::to_owned),
        functional_args,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }))
}

#[test]
fn functional_index_admits_only_explicit_builtin_lowercase_string_ops() {
    for functional_op in ["lower", "upper", "trim", "length"] {
        let operation = create_index(Some("functional"), Some(functional_op), None);
        assert!(operation.validate().is_ok(), "op {functional_op} refused");
    }
    for functional_op in ["substring", "mod", "coalesce", "concat", "reverse"] {
        let operation = create_index(Some("functional"), Some(functional_op), None);
        assert_eq!(
            operation.validate(),
            Err(SchemaMigrationError::UnsafeOperation(
                "index function is outside the built-in functional-index allowlist"
            )),
            "op {functional_op} admitted"
        );
    }
}

#[test]
fn functional_index_requires_an_explicit_builtin_operation() {
    let operation = create_index(Some("functional"), None, None);
    assert_eq!(
        operation.validate(),
        Err(SchemaMigrationError::UnsafeOperation(
            "functional index requires an explicit built-in operation"
        ))
    );
}

#[test]
fn functional_args_are_refused_even_when_empty() {
    for functional_args in [
        Some(Vec::new()),
        Some(vec![QueryValue::List(vec![QueryValue::Int(1)])]),
    ] {
        let operation = create_index(Some("functional"), Some("lower"), functional_args);
        assert_eq!(
            operation.validate(),
            Err(SchemaMigrationError::UnsafeOperation(
                "functional_args are not admitted because functional indexes do not use them"
            )),
            "args admitted"
        );
    }
}

#[test]
fn functional_options_on_nonfunctional_or_omitted_families_are_refused_first() {
    for index_type in [None, Some("btree"), Some("fts"), Some("vector")] {
        for (functional_op, functional_args) in [(Some("lower"), None), (None, Some(Vec::new()))] {
            let operation = create_index(index_type, functional_op, functional_args);
            assert_eq!(
                operation.validate(),
                Err(SchemaMigrationError::UnsafeOperation(
                    "functional_op/functional_args are only valid for 'functional' indexes"
                )),
                "index_type {index_type:?} admitted functional options"
            );
        }
    }
}

#[test]
fn batch_admission_rejects_request_cancellation() {
    let op = crate::batch::BatchOp::CancelRequest(crate::admin::RequestExecId(42));
    assert!(matches!(
        SchemaMigrationOp::try_from(op),
        Err(SchemaMigrationError::UnsupportedOperation(
            "request cancellation"
        ))
    ));
}
