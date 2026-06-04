use crate::batch::Batch;
use crate::ddl;
use crate::query::Query;
use shamir_query_types::admin::GroupRef;

/// Helper: build two batches — one using the first-class DDL method, one using
/// `b.op()` — and assert their serialized JSON is identical.
fn assert_same_wire(first_class: impl FnOnce(&mut Batch), escape_hatch: impl FnOnce(&mut Batch)) {
    let mut b1 = Batch::new();
    first_class(&mut b1);
    let j1 = serde_json::to_value(b1.build()).unwrap();

    let mut b2 = Batch::new();
    escape_hatch(&mut b2);
    let j2 = serde_json::to_value(b2.build()).unwrap();

    assert_eq!(j1, j2);
}

// ============================================================================
// 1. create_table
// ============================================================================

#[test]
fn ddl_create_table_matches_op() {
    assert_same_wire(
        |b| {
            b.create_table("mk", ddl::create_table("users").repo("main"));
        },
        |b| {
            b.op("mk", ddl::create_table("users").repo("main"));
        },
    );
}

// ============================================================================
// 2. create_index
// ============================================================================

#[test]
fn ddl_create_index_matches_op() {
    assert_same_wire(
        |b| {
            b.create_index(
                "idx",
                ddl::create_index("email_idx", "users")
                    .field("email")
                    .unique()
                    .repo("main"),
            );
        },
        |b| {
            b.op(
                "idx",
                ddl::create_index("email_idx", "users")
                    .field("email")
                    .unique()
                    .repo("main"),
            );
        },
    );
}

// ============================================================================
// 3. create_function
// ============================================================================

#[test]
fn ddl_create_function_matches_op() {
    assert_same_wire(
        |b| {
            b.create_function("fn", ddl::create_function("my_fn").source("fn main(){}"));
        },
        |b| {
            b.op("fn", ddl::create_function("my_fn").source("fn main(){}"));
        },
    );
}

// ============================================================================
// 4. create_validator + bind_validator
// ============================================================================

#[test]
fn ddl_create_validator_matches_op() {
    assert_same_wire(
        |b| {
            b.create_validator("val", ddl::create_validator("check").wasm("AAAA"));
        },
        |b| {
            b.op("val", ddl::create_validator("check").wasm("AAAA"));
        },
    );
}

#[test]
fn ddl_bind_validator_matches_op() {
    assert_same_wire(
        |b| {
            b.bind_validator(
                "bind",
                ddl::bind_validator("check", "users")
                    .repo("main")
                    .ops([ddl::WriteOp::Insert])
                    .priority(5000),
            );
        },
        |b| {
            b.op(
                "bind",
                ddl::bind_validator("check", "users")
                    .repo("main")
                    .ops([ddl::WriteOp::Insert])
                    .priority(5000),
            );
        },
    );
}

// ============================================================================
// 5. chmod
// ============================================================================

#[test]
fn ddl_chmod_matches_op() {
    assert_same_wire(
        |b| {
            b.chmod(
                "perm",
                ddl::chmod(ddl::res::table("app", "main", "users"), 0o750),
            );
        },
        |b| {
            b.op(
                "perm",
                ddl::chmod(ddl::res::table("app", "main", "users"), 0o750),
            );
        },
    );
}

// ============================================================================
// 6. create_user
// ============================================================================

#[test]
fn ddl_create_user_matches_op() {
    assert_same_wire(
        |b| {
            b.create_user("usr", ddl::create_user("alice", "s3cret").roles(["admin"]));
        },
        |b| {
            b.op("usr", ddl::create_user("alice", "s3cret").roles(["admin"]));
        },
    );
}

// ============================================================================
// 7. create_group + add_group_member
// ============================================================================

#[test]
fn ddl_create_group_matches_op() {
    assert_same_wire(
        |b| {
            b.create_group("grp", ddl::create_group("devs"));
        },
        |b| {
            b.op("grp", ddl::create_group("devs"));
        },
    );
}

#[test]
fn ddl_add_group_member_matches_op() {
    assert_same_wire(
        |b| {
            b.add_group_member(
                "add",
                ddl::add_group_member(
                    GroupRef::Name {
                        name: "devs".into(),
                    },
                    42,
                ),
            );
        },
        |b| {
            b.op(
                "add",
                ddl::add_group_member(
                    GroupRef::Name {
                        name: "devs".into(),
                    },
                    42,
                ),
            );
        },
    );
}

// ============================================================================
// 8. start_migration
// ============================================================================

#[test]
fn ddl_start_migration_matches_op() {
    assert_same_wire(
        |b| {
            b.start_migration("mig", ddl::start_migration("events", "archive", "fjall"));
        },
        |b| {
            b.op("mig", ddl::start_migration("events", "archive", "fjall"));
        },
    );
}

// ============================================================================
// 9. access_tree
// ============================================================================

#[test]
fn ddl_access_tree_matches_op() {
    assert_same_wire(
        |b| {
            b.access_tree("tree", ddl::access_tree().depth(3));
        },
        |b| {
            b.op("tree", ddl::access_tree().depth(3));
        },
    );
}

// ============================================================================
// 10. list_databases
// ============================================================================

#[test]
fn ddl_list_databases_matches_op() {
    assert_same_wire(
        |b| {
            b.list_databases("dbs", ddl::list_databases());
        },
        |b| {
            b.op("dbs", ddl::list_databases());
        },
    );
}

// ============================================================================
// End-to-end: mixed DDL + DML batch
// ============================================================================

#[test]
fn mixed_ddl_and_dml_batch() {
    // Build via first-class methods
    let mut b1 = Batch::named("setup");
    b1.create_table("mk_tbl", ddl::create_table("users").repo("main"));
    b1.create_index(
        "mk_idx",
        ddl::create_index("email_idx", "users")
            .field("email")
            .unique()
            .repo("main"),
    );
    b1.chmod(
        "perm",
        ddl::chmod(ddl::res::table("app", "main", "users"), 0o750),
    );
    b1.query("q", Query::from("users").select(["id"]));

    // Build via escape hatch
    let mut b2 = Batch::named("setup");
    b2.op("mk_tbl", ddl::create_table("users").repo("main"));
    b2.op(
        "mk_idx",
        ddl::create_index("email_idx", "users")
            .field("email")
            .unique()
            .repo("main"),
    );
    b2.op(
        "perm",
        ddl::chmod(ddl::res::table("app", "main", "users"), 0o750),
    );
    b2.query("q", Query::from("users").select(["id"]));

    let j1 = serde_json::to_value(b1.build()).unwrap();
    let j2 = serde_json::to_value(b2.build()).unwrap();
    assert_eq!(j1, j2);

    // Verify structure: 4 entries, all return_result = true
    let queries = j1["queries"].as_object().unwrap();
    assert_eq!(queries.len(), 4);
    for (_alias, entry) in queries {
        assert_eq!(entry["return_result"], true);
    }
}

// ============================================================================
// Handle is returned and usable
// ============================================================================

#[test]
fn ddl_methods_return_handle() {
    let mut b = Batch::new();
    let h = b.create_table("mk_tbl", ddl::create_table("users"));
    assert_eq!(h.alias(), "mk_tbl");
}
