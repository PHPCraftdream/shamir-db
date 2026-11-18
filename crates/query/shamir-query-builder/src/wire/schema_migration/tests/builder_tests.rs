use crate::wire::schema_migration::{SchemaMigration, SchemaMigrationSet};
use shamir_query_types::admin::types::schema_migration::{SchemaMigrationError, SchemaMigrationOp};
use shamir_query_types::batch::BatchOp;
use shamir_types::types::value::QueryValue;

fn body(
    id: &str,
    ordinal: u64,
) -> shamir_query_types::admin::types::schema_migration::SchemaMigrationBody {
    SchemaMigration::new(id, ordinal, id).build().unwrap()
}

#[test]
fn builder_accepts_typed_ddl_and_rejects_excluded_operations() {
    let body = SchemaMigration::new("create", 1, "Create table")
        .op(crate::ddl::create_table("users")
            .schema([crate::ddl::field(["email"]).string().build()])
            .build())
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(body.operations.len(), 1);
    assert!(SchemaMigration::new("excluded", 1, "Excluded")
        .op(BatchOp::Call(crate::CallOp::new(
            "run",
            Vec::<shamir_query_types::filter::FilterValue>::new()
        )))
        .is_err());
}

#[test]
fn set_rejects_gaps_duplicate_ids_and_duplicate_ordinals() {
    assert!(SchemaMigrationSet::new(vec![body("a", 1), body("b", 3)]).is_err());
    assert!(SchemaMigrationSet::new(vec![body("a", 1), body("a", 2)]).is_err());
    assert!(SchemaMigrationSet::new(vec![body("a", 1), body("b", 1)]).is_err());
}

#[test]
fn builder_rejects_unknown_body_version() {
    assert!(SchemaMigration::new("a", 1, "a")
        .version(2)
        .build()
        .is_err());
}

/// One typed `create_index` batch op produced by the no-args functional
/// constructor, with the operation and arguments under test written into
/// the typed model fields directly (the typed constructor normalizes an
/// empty argument list to absent, which would hide the `[]` case).
fn functional_index_op(functional_op: &str, args: Option<Vec<QueryValue>>) -> BatchOp {
    let BatchOp::CreateIndex(mut op) = crate::ddl::create_index("ix_email", "users")
        .functional(vec![String::from("email")], "lower")
        .expect("typed functional constructor builds")
    else {
        panic!("the functional constructor builds a create_index operation")
    };
    op.functional_op = Some(functional_op.to_string());
    op.functional_args = args;
    BatchOp::CreateIndex(op)
}

/// One migration body carrying a single functional `create_index`.
fn functional_body(
    functional_op: &str,
    args: Option<Vec<QueryValue>>,
) -> Result<
    shamir_query_types::admin::types::schema_migration::SchemaMigrationBody,
    SchemaMigrationError,
> {
    SchemaMigration::new("mig_func", 1, "mig_func")
        .op(functional_index_op(functional_op, args))
        .and_then(|builder| builder.build())
}

/// The migration model admits exactly the four functional operations the
/// server demonstrably builds and uses (`lower`, `upper`, `trim`,
/// `length`), with NO arguments: each constructs a typed body locally,
/// keeping the operation name and the indexed field.
#[test]
fn builder_admits_built_in_functional_ops_without_args() {
    for functional_op in ["lower", "upper", "trim", "length"] {
        let body = SchemaMigration::new("mig_func", 1, "mig_func")
            .op(crate::ddl::create_index("ix_email", "users")
                .functional(vec![String::from("email")], functional_op)
                .expect("typed functional constructor builds"))
            .unwrap()
            .build()
            .unwrap_or_else(|error| panic!("{functional_op} must be admitted, got {error:?}"));
        let SchemaMigrationOp::CreateIndex(create_index) = &body.operations[0] else {
            panic!("expected a typed create_index operation")
        };
        assert_eq!(create_index.functional_op.as_deref(), Some(functional_op));
        assert_eq!(create_index.fields, vec![vec![String::from("email")]]);
    }
}

/// Refuses each remaining allowlist-era name (`substring`, `mod`,
/// `coalesce`, `concat`) with the model's exact typed reason, with and
/// without arguments.
#[test]
fn builder_refuses_unbuildable_functional_ops_with_and_without_args() {
    for functional_op in ["substring", "mod", "coalesce", "concat"] {
        for args in [None, Some(vec![QueryValue::from("arg")])] {
            let error = functional_body(functional_op, args)
                .expect_err(&format!("{functional_op} must be refused"));
            let SchemaMigrationError::UnsafeOperation(reason) = &error else {
                panic!("typed refusal, got {error:?}")
            };
            assert_eq!(
                *reason, "index function is outside the built-in functional-index allowlist",
                "for {functional_op}"
            );
        }
    }
}

/// `functional_args` on an admitted op (even empty) is a typed refusal,
/// never silently dropped, with the model's exact reason.
#[test]
fn builder_refuses_functional_args_on_admitted_ops_even_empty() {
    for functional_op in ["lower", "upper", "trim", "length"] {
        for args in [Some(vec![QueryValue::from("arg")]), Some(Vec::new())] {
            let error = functional_body(functional_op, args)
                .expect_err("functional_args on an admitted op must be refused");
            let SchemaMigrationError::UnsafeOperation(reason) = &error else {
                panic!("typed refusal, got {error:?}")
            };
            assert_eq!(
                *reason,
                "functional_args are not admitted because functional indexes do not use them",
                "for {functional_op}"
            );
        }
    }
}

#[cfg(feature = "crypto")]
#[test]
fn set_preserves_order_but_dependency_order_is_canonical() {
    use shamir_query_types::admin::types::schema_migration::SchemaMigrationFunctionDependencies;
    let first = SchemaMigration::new("a", 1, "a")
        .dependencies(vec![SchemaMigrationFunctionDependencies {
            function: "f".into(),
            depends_on: vec!["z".into(), "a".into()],
        }])
        .build()
        .unwrap();
    let set = SchemaMigrationSet::new(vec![first, body("b", 2)]).unwrap();
    let bytes = set.canonical_bytes().unwrap();
    assert_eq!(bytes.len(), 2);
    assert_eq!(set.checksums().unwrap().len(), 2);
    assert_ne!(bytes[0], bytes[1]);
}
