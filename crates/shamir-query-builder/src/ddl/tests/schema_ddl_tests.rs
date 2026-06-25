//! Tests for Database, Repository, Table, Index, Buffer config DDL constructors,
//! and `create_db` with `if_not_exists`.

use shamir_query_types::admin::{BufferConfigDto, BufferConfigPatch, FkAction, ForeignKeyDto};
use shamir_types::mpack;

use crate::ddl;

use super::helpers::roundtrip;

// ============================================================================
// Database DDL
// ============================================================================

#[test]
fn create_db_wire() {
    let op = ddl::create_db("mydb").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_db": "mydb"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn drop_db_no_hmac() {
    let op = ddl::drop_db("mydb").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_db": "mydb"
        })
    );
}

#[test]
fn drop_db_with_hmac() {
    let op = ddl::drop_db("mydb").hmac("abc123").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_db": "mydb",
            "hmac": "abc123"
        })
    );
}

// ============================================================================
// Repository DDL
// ============================================================================

#[test]
fn create_repo_minimal() {
    let op = ddl::create_repo("hot_cache").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_repo": "hot_cache"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_repo_full() {
    let op = ddl::create_repo("hot_cache")
        .engine("in_memory")
        .tables(["sessions", "tokens"])
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_repo": "hot_cache",
            "engine": "in_memory",
            "tables": ["sessions", "tokens"]
        })
    );
}

#[test]
fn drop_repo_wire() {
    let op = ddl::drop_repo("temp").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_repo": "temp"
        })
    );
}

// ============================================================================
// Table DDL
// ============================================================================

#[test]
fn create_table_default_repo() {
    let op = ddl::create_table("products").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_table": "products",
            "repo": "main"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_table_custom_repo() {
    let op = ddl::create_table("products").repo("hot").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_table": "products",
            "repo": "hot"
        })
    );
}

#[test]
fn drop_table_default() {
    let op = ddl::drop_table("users").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_table": "users",
            "repo": "main"
        })
    );
}

#[test]
fn drop_table_with_hmac() {
    let op = ddl::drop_table("users").hmac("ff00").repo("cold").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_table": "users",
            "repo": "cold",
            "hmac": "ff00"
        })
    );
}

// ============================================================================
// Index DDL
// ============================================================================

#[test]
fn create_index_regular() {
    let op = ddl::create_index("name_idx", "users")
        .fields(vec![vec!["name".into()]])
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_index": "name_idx",
            "table": "users",
            "fields": [["name"]],
            "unique": false,
            "sorted": false,
            "repo": "main"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_index_unique() {
    let op = ddl::create_index("email_idx", "users")
        .field("email")
        .unique()
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["create_index"], "email_idx");
    assert_eq!(j["unique"], true);
    assert_eq!(j["fields"], mpack!([["email"]]));
}

#[test]
fn create_index_fts() {
    let op = ddl::create_index("body_fts", "posts")
        .field("body")
        .index_type("fts")
        .fts_tokenizer("unicode")
        .fts_language("en")
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["index_type"], "fts");
    assert_eq!(j["fts_tokenizer"], "unicode");
    assert_eq!(j["fts_language"], "en");
}

#[test]
fn create_index_vector() {
    let op = ddl::create_index("embed_idx", "docs")
        .field("embedding")
        .index_type("vector")
        .vector_dim(128)
        .vector_metric("cosine")
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["index_type"], "vector");
    assert_eq!(j["vector_dim"], 128);
    assert_eq!(j["vector_metric"], "cosine");
}

#[test]
fn create_index_functional() {
    let op = ddl::create_index("lower_name", "users")
        .field("name")
        .index_type("functional")
        .functional_op("lower")
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["index_type"], "functional");
    assert_eq!(j["functional_op"], "lower");
}

/// Builder `.functional_args(vec![...])` accepts `QueryValue` args and they
/// survive a msgpack round-trip with the correct scalar shapes.
#[test]
fn create_index_functional_with_args() {
    let op = ddl::create_index("mod_price", "items")
        .field("price")
        .index_type("functional")
        .functional_op("mod")
        .functional_args(vec![mpack!(10), mpack!("base")])
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["functional_op"], "mod");
    // Integer arg must round-trip as a number (not null, not string).
    assert_eq!(j["functional_args"], mpack!([10, "base"]));
}

/// When `functional_args` is not set, the field must be absent in the wire encoding.
#[test]
fn create_index_functional_args_absent_when_none() {
    let op = ddl::create_index("lower_name", "users")
        .field("name")
        .index_type("functional")
        .functional_op("lower")
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let j: shamir_types::types::value::QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert!(
        j.get("functional_args").is_none(),
        "functional_args must be absent when not set, got: {j:?}"
    );
}

#[test]
fn create_index_sorted() {
    let op = ddl::create_index("price_sorted", "products")
        .field("price")
        .sorted()
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["sorted"], true);
    assert_eq!(j["unique"], false);
}

#[test]
fn create_index_sorted_with_include() {
    let op = ddl::create_index("score_sorted", "users")
        .field("score")
        .sorted()
        .include(vec![vec!["email".to_string()], vec!["name".to_string()]])
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["sorted"], true);
    assert_eq!(j["include"], mpack!([["email"], ["name"]]));
    // `include` must be absent when empty (skip_serializing_if).
    let op_no_include = ddl::create_index("score_sorted2", "users")
        .field("score")
        .sorted()
        .build();
    let bytes2 = rmp_serde::to_vec_named(&op_no_include).unwrap();
    let j2: shamir_types::types::value::QueryValue = rmp_serde::from_slice(&bytes2).unwrap();
    assert!(
        j2.get("include").is_none(),
        "empty include should be omitted from msgpack encoding"
    );
}

#[test]
fn drop_index_wire() {
    let op = ddl::drop_index("name_idx", "users").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_index": "name_idx",
            "table": "users",
            "unique": false,
            "repo": "main"
        })
    );
}

#[test]
fn drop_index_unique_with_hmac() {
    let op = ddl::drop_index("email_idx", "users")
        .unique()
        .hmac("dead")
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["unique"], true);
    assert_eq!(j["hmac"], "dead");
}

// ============================================================================
// Buffer config DDL
// ============================================================================

#[test]
fn set_buffer_config_wire() {
    let cfg = BufferConfigDto {
        max_bytes: 1024,
        max_entries: 100,
        ttl_ms: Some(5000),
        flush_interval_ms: 500,
        flush_batch_size: 50,
    };
    let op = ddl::set_buffer_config("users", cfg).build();
    let j = roundtrip(&op);
    assert_eq!(j["set_buffer_config"], "users");
    assert_eq!(j["repo"], "main");
    assert_eq!(j["config"]["max_bytes"], 1024);
    assert_eq!(j["config"]["ttl_ms"], 5000);
}

#[test]
fn get_buffer_config_wire() {
    let op = ddl::get_buffer_config("users").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "get_buffer_config": "users",
            "repo": "main"
        })
    );
}

#[test]
fn alter_buffer_config_wire() {
    let patch = BufferConfigPatch {
        max_bytes: Some(2048),
        ..Default::default()
    };
    let op = ddl::alter_buffer_config("users", patch)
        .repo("cold")
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["alter_buffer_config"], "users");
    assert_eq!(j["repo"], "cold");
    assert_eq!(j["patch"]["max_bytes"], 2048);
}

// ============================================================================
// create_db with if_not_exists
// ============================================================================

#[test]
fn create_db_if_not_exists_wire() {
    let op = ddl::create_db("newdb").if_not_exists().build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_db": "newdb",
            "if_not_exists": true
        })
    );
    assert!(op.is_admin());
}

// ============================================================================
// FieldBuilder constraints — one_of
// ============================================================================

/// `one_of` values survive a msgpack round-trip with the correct scalar shapes.
#[test]
fn field_one_of_wire() {
    let op = ddl::add_schema_rule("users")
        .rule(
            ddl::field(["status"])
                .string()
                .one_of(vec![mpack!("active"), mpack!("archived")]),
        )
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["rule"]["one_of"], mpack!(["active", "archived"]));
}

/// `one_of` is absent from the wire encoding when not set.
#[test]
fn field_one_of_absent_when_none() {
    let op = ddl::add_schema_rule("users")
        .rule(ddl::field(["status"]).string())
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let j: shamir_types::types::value::QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert!(
        j["rule"].get("one_of").is_none(),
        "one_of must be absent when not set, got: {j:?}"
    );
}

// ============================================================================
// FieldBuilder constraints — default (Phase ②.4b — surface only)
// ============================================================================

/// `.default(Int(5))` emits a top-level `"default": 5` on the wire (mirrors
/// how `one_of` travels). Surface only — INSERT-path stamp is ②.4c, not here.
#[test]
fn field_default_int_wire() {
    let op = ddl::add_schema_rule("users")
        .rule(ddl::field(["count"]).int().default(mpack!(5)))
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["rule"]["default"], mpack!(5));
}

/// `.default(Str(...))` carries the string shape (any QueryValue variant).
#[test]
fn field_default_str_wire() {
    let op = ddl::add_schema_rule("users")
        .rule(ddl::field(["role"]).string().default(mpack!("guest")))
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["rule"]["default"], mpack!("guest"));
}

/// `default` is absent from the wire encoding when not set (additive —
/// rules written before ②.4b keep their byte-identical shape).
#[test]
fn field_default_absent_when_none() {
    let op = ddl::add_schema_rule("users")
        .rule(ddl::field(["status"]).string())
        .build();
    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let j: shamir_types::types::value::QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    assert!(
        j["rule"].get("default").is_none(),
        "default must be absent when not set, got: {j:?}"
    );
}

// ============================================================================
// FieldBuilder type-tag setters — set / null_type
// ============================================================================

/// `.set()` produces the `"set"` type tag in the wire encoding.
#[test]
fn field_set_type_tag_wire() {
    let op = ddl::add_schema_rule("users")
        .rule(ddl::field(["tags"]).set())
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["rule"]["type"], "set");
}

/// `.null_type()` produces the `"null"` type tag in the wire encoding.
#[test]
fn field_null_type_tag_wire() {
    let op = ddl::add_schema_rule("users")
        .rule(ddl::field(["erased"]).null_type())
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["rule"]["type"], "null");
}

// ============================================================================
// Foreign-key builder (Phase C2 / D / ②.2a)
// ============================================================================

/// `foreign_key(...)` sets `on_delete = Restrict` (safe-by-default) and
/// `on_update = NoAction` (additive — existing FK callers keep current
/// behavior). Regression: adding the `on_update` field must not change the
/// observable wire shape for plain FKs.
#[test]
fn foreign_key_defaults_on_delete_restrict_on_update_no_action() {
    let rule = ddl::field(["parent_id"])
        .int()
        .foreign_key("parent", "id")
        .build();
    assert_eq!(
        rule.constraints.foreign_key,
        Some(ForeignKeyDto {
            ref_table: "parent".to_string(),
            ref_field: "id".to_string(),
            on_delete: FkAction::Restrict,
            on_update: FkAction::NoAction,
        })
    );
}

/// `foreign_key_on_delete(...)` sets `on_update = NoAction` (additive).
#[test]
fn foreign_key_on_delete_sets_on_update_no_action() {
    let rule = ddl::field(["parent_id"])
        .int()
        .foreign_key_on_delete("parent", "id", FkAction::Cascade)
        .build();
    assert_eq!(
        rule.constraints.foreign_key,
        Some(ForeignKeyDto {
            ref_table: "parent".to_string(),
            ref_field: "id".to_string(),
            on_delete: FkAction::Cascade,
            on_update: FkAction::NoAction,
        })
    );
}

/// `foreign_key_on_update(...)` (Phase ②.2a) sets `on_delete = Restrict`
/// (safe-by-default) and `on_update` from the argument.
#[test]
fn foreign_key_on_update_sets_on_delete_restrict() {
    let rule = ddl::field(["parent_id"])
        .int()
        .foreign_key_on_update("parent", "id", FkAction::Cascade)
        .build();
    assert_eq!(
        rule.constraints.foreign_key,
        Some(ForeignKeyDto {
            ref_table: "parent".to_string(),
            ref_field: "id".to_string(),
            on_delete: FkAction::Restrict,
            on_update: FkAction::Cascade,
        })
    );
}

/// `foreign_key_with_actions(...)` (Phase ②.2a) sets both actions explicitly.
#[test]
fn foreign_key_with_actions_sets_both() {
    let rule = ddl::field(["parent_id"])
        .int()
        .foreign_key_with_actions("parent", "id", FkAction::SetNull, FkAction::Cascade)
        .build();
    assert_eq!(
        rule.constraints.foreign_key,
        Some(ForeignKeyDto {
            ref_table: "parent".to_string(),
            ref_field: "id".to_string(),
            on_delete: FkAction::SetNull,
            on_update: FkAction::Cascade,
        })
    );
}
