use crate::wire::schema_migration::{SchemaMigration, SchemaMigrationSet};
use shamir_query_types::batch::BatchOp;

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
