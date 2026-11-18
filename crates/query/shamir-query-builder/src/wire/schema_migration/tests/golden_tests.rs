use crate::wire::schema_migration::{SchemaMigration, SchemaMigrationSet};
use shamir_query_types::admin::types::schema_migration::SchemaMigrationFunctionDependencies;

const FIXTURE: &str =
    include_str!("../../../../../shamir-query-types/test-vectors/schema_migration_v1.json");

fn fixture_hex(key: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    value["vectors"][0][key].as_str().unwrap().to_owned()
}

fn all_operations() -> Vec<shamir_query_types::batch::BatchOp> {
    use crate::ddl;
    vec![
        ddl::create_table("t").build(),
        ddl::drop_table("t")
            .hmac("tag")
            .if_exists()
            .build()
            .unwrap(),
        ddl::rename_table("t", "u").build(),
        ddl::create_index("ix", "t")
            .fts(vec!["a".to_owned()], crate::ddl::Tokenizer::Whitespace)
            .unwrap(),
        ddl::drop_index("ix", "t")
            .hmac("tag")
            .if_exists()
            .build()
            .unwrap(),
        ddl::rename_index("t", "ix", "iy").build(),
        ddl::set_table_schema("t").build(),
        ddl::add_schema_rule("t")
            .rule(ddl::field(["a"]).f64_type().min_f64(1.0).max_f64(-0.0))
            .build()
            .unwrap(),
        ddl::remove_schema_rule("t", ["a"]).build(),
        ddl::create_function("f")
            .source("fn f() {}")
            .build()
            .unwrap(),
        ddl::drop_function("f")
            .hmac("tag")
            .if_exists()
            .build()
            .unwrap(),
        ddl::rename_function("f", "g"),
        ddl::check_function().source("fn f() {}").build().unwrap(),
        ddl::alter_function("f", "never"),
        ddl::create_validator("v")
            .source("fn validate() {}")
            .build()
            .unwrap(),
        ddl::drop_validator("v")
            .hmac("tag")
            .if_exists()
            .build()
            .unwrap(),
        ddl::rename_validator("v", "w"),
        ddl::bind_validator("v", "t")
            .db("db")
            .ops([ddl::WriteOp::Insert, ddl::WriteOp::Update])
            .build(),
        ddl::unbind_validator("v", "t").db("db").build(),
    ]
}

fn fixture_body() -> shamir_query_types::admin::types::schema_migration::SchemaMigrationBody {
    let dependencies = Vec::<SchemaMigrationFunctionDependencies>::new();
    all_operations()
        .into_iter()
        .try_fold(
            SchemaMigration::new("id", 1, "n").dependencies(dependencies),
            |builder, operation| builder.op(operation),
        )
        .unwrap()
        .build()
        .unwrap()
}

#[test]
fn golden_body_and_checksum_match_independent_reference() {
    let set = SchemaMigrationSet::new(vec![fixture_body()]).unwrap();
    assert_eq!(
        hex(&set.canonical_bytes().unwrap()[0]),
        fixture_hex("expected_canonical_bytes_hex")
    );
    assert_eq!(
        hex(&set.checksums().unwrap()[0]),
        fixture_hex("expected_checksum_hex")
    );
}

#[test]
fn request_id_is_excluded_from_canonical_body() {
    use crate::ddl;
    use shamir_types::types::record_id::RecordId;
    let make = |request_id| {
        let drop = ddl::drop_index("ix", "t").hmac("tag");
        let drop = if let Some(id) = request_id {
            drop.request_id(id)
        } else {
            drop
        };
        SchemaMigration::new("id", 1, "n")
            .op(drop.build().unwrap())
            .unwrap()
            .build()
            .unwrap()
    };
    assert_eq!(
        shamir_query_types::admin::types::schema_migration::canonical_body_checksum(&make(None))
            .unwrap(),
        shamir_query_types::admin::types::schema_migration::canonical_body_checksum(&make(Some(
            RecordId([7; 16])
        )))
        .unwrap()
    );
}

#[test]
fn msgpack_round_trip_preserves_checksum_and_nested_binary_values() {
    // Serde round-trip is the wire-format subject; DDL construction remains typed.
    use shamir_types::types::value::QueryValue;
    let nested = QueryValue::List(vec![
        QueryValue::Map(shamir_collections::new_map()),
        QueryValue::Bin(vec![0, 255, 1]),
        QueryValue::List(vec![QueryValue::Bin(vec![4, 5])]),
    ]);
    // Nested values travel through the schema constraint `one_of`: functional index
    // arguments are refused by admission and cannot carry this coverage.
    let op = crate::ddl::add_schema_rule("t")
        .rule(crate::ddl::field(["a"]).one_of(vec![nested]))
        .build()
        .unwrap();
    let body = SchemaMigration::new("bin", 1, "binary")
        .op(op)
        .unwrap()
        .build()
        .unwrap();
    let checksum =
        shamir_query_types::admin::types::schema_migration::canonical_body_checksum(&body).unwrap();
    let encoded = rmp_serde::to_vec_named(&body).unwrap();
    let decoded: shamir_query_types::admin::types::schema_migration::SchemaMigrationBody =
        rmp_serde::from_slice(&encoded).unwrap();
    assert_eq!(
        shamir_query_types::admin::types::schema_migration::canonical_body_checksum(&decoded)
            .unwrap(),
        checksum
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
