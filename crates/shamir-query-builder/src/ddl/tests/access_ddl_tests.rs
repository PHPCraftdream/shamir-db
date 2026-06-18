//! Tests for Access tree, chmod / chown / chgrp, Group DDL, Auth users / roles,
//! and `res` helpers.

use shamir_query_types::auth::{Action, Effect, Permission, Resource};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::ddl;

use super::helpers::roundtrip;

// ============================================================================
// Access tree
// ============================================================================

#[test]
fn access_tree_defaults() {
    let op = ddl::access_tree().build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "access_tree": true
        })
    );
    assert!(op.is_admin());
}

#[test]
fn access_tree_with_depth() {
    let op = ddl::access_tree().depth(2).build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "access_tree": true,
            "depth": 2
        })
    );
}

#[test]
fn access_tree_with_db() {
    let op = ddl::access_tree().db("mydb").depth(1).build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "access_tree": true,
            "depth": 1,
            "db": "mydb"
        })
    );
}

// ============================================================================
// chmod / chown / chgrp
// ============================================================================

#[test]
fn chmod_table_wire() {
    let op = ddl::chmod(ddl::res::table("mydb", "main", "users"), 0o700);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chmod": {
                "table": ["mydb", "main", "users"]
            },
            "mode": 448
        })
    );
    assert!(op.is_admin());
}

#[test]
fn chmod_function_namespace_wire() {
    let op = ddl::chmod(ddl::res::function_namespace(), 0o755);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chmod": {
                "function_namespace": true
            },
            "mode": 493
        })
    );
}

#[test]
fn chown_database_wire() {
    let op = ddl::chown(ddl::res::database("testdb"), 7);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chown": {
                "database": "testdb"
            },
            "owner": 7
        })
    );
    assert!(op.is_admin());
}

#[test]
fn chown_function_wire() {
    let op = ddl::chown(ddl::res::function("my_fn"), 10);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chown": {
                "function": "my_fn"
            },
            "owner": 10
        })
    );
}

#[test]
fn chgrp_store_wire() {
    let op = ddl::chgrp(ddl::res::store("testdb", "main"), Some(3));
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chgrp": {
                "store": ["testdb", "main"]
            },
            "group": 3
        })
    );
}

#[test]
fn chgrp_null_group_wire() {
    let op = ddl::chgrp(ddl::res::database("testdb"), None);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chgrp": {
                "database": "testdb"
            },
            "group": null
        })
    );
}

// ============================================================================
// Group DDL
// ============================================================================

#[test]
fn create_group_wire() {
    let op = ddl::create_group("devs");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_group": "devs"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn drop_group_by_name_wire() {
    let op = ddl::drop_group(ddl::GroupRef::Name {
        name: "devs".into(),
    });
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_group": {
                "name": "devs"
            }
        })
    );
}

#[test]
fn drop_group_by_id_wire() {
    let op = ddl::drop_group(ddl::GroupRef::Id { id: 3 });
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_group": {
                "id": 3
            }
        })
    );
}

#[test]
fn add_group_member_wire() {
    let op = ddl::add_group_member(
        ddl::GroupRef::Name {
            name: "devs".into(),
        },
        42,
    );
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "add_group_member": {
                "name": "devs"
            },
            "user": 42
        })
    );
    assert!(op.is_admin());
}

#[test]
fn remove_group_member_wire() {
    let op = ddl::remove_group_member(ddl::GroupRef::Id { id: 1 }, 42);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "remove_group_member": {
                "id": 1
            },
            "user": 42
        })
    );
}

// ============================================================================
// Auth DDL (users + roles)
// ============================================================================

#[test]
fn create_user_minimal() {
    let op = ddl::create_user("alice", "s3cret").build();
    let j = roundtrip(&op);
    assert_eq!(j["create_user"], "alice");
    assert_eq!(j["password"], "s3cret");
    assert_eq!(j["roles"], mpack!([]));
    assert!(op.is_admin());
}

#[test]
fn create_user_full() {
    let op = ddl::create_user("bob", "hunter2")
        .roles(["admin", "viewer"])
        .profile(mpack!({"department": "eng"}))
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["create_user"], "bob");
    assert_eq!(j["roles"], mpack!(["admin", "viewer"]));
    assert_eq!(j["profile"], mpack!({"department": "eng"}));
}

#[test]
fn drop_user_wire() {
    let op = ddl::drop_user("alice").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_user": "alice"
        })
    );
}

#[test]
fn drop_user_with_hmac() {
    let op = ddl::drop_user("alice").hmac("abc").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_user": "alice",
            "hmac": "abc"
        })
    );
}

#[test]
fn create_role_wire() {
    let perms = vec![Permission {
        effect: Effect::Allow,
        actions: vec![Action::Read],
        resource: Resource::Global,
        row_filter: None,
    }];
    let op = ddl::create_role("viewer", perms);
    let j = roundtrip(&op);
    assert_eq!(j["create_role"], "viewer");
    assert_eq!(j["permissions"][0]["effect"], "allow");
    assert_eq!(j["permissions"][0]["actions"], mpack!(["read"]));
    assert!(op.is_admin());
}

#[test]
fn drop_role_wire() {
    let op = ddl::drop_role("viewer").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_role": "viewer"
        })
    );
}

#[test]
fn drop_role_with_hmac() {
    let op = ddl::drop_role("viewer").hmac("ff").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_role": "viewer",
            "hmac": "ff"
        })
    );
}

#[test]
fn grant_role_wire() {
    let op = ddl::grant_role("admin", "alice");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "grant_role": "admin",
            "user": "alice"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn revoke_role_wire() {
    let op = ddl::revoke_role("admin", "alice");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "revoke_role": "admin",
            "user": "alice"
        })
    );
}

// ============================================================================
// res helpers
// ============================================================================

fn res_to_wire<T: serde::Serialize>(r: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(r).expect("serialize");
    rmp_serde::from_slice(&bytes).expect("decode")
}

#[test]
fn res_database() {
    let r = ddl::res::database("mydb");
    assert_eq!(res_to_wire(&r), mpack!({"database": "mydb"}));
}

#[test]
fn res_store() {
    let r = ddl::res::store("mydb", "main");
    assert_eq!(res_to_wire(&r), mpack!({"store": ["mydb", "main"]}));
}

#[test]
fn res_table() {
    let r = ddl::res::table("mydb", "main", "users");
    assert_eq!(
        res_to_wire(&r),
        mpack!({"table": ["mydb", "main", "users"]})
    );
}

#[test]
fn res_function() {
    let r = ddl::res::function("my_fn");
    assert_eq!(res_to_wire(&r), mpack!({"function": "my_fn"}));
}

#[test]
fn res_function_namespace() {
    let r = ddl::res::function_namespace();
    assert_eq!(res_to_wire(&r), mpack!({"function_namespace": true}));
}
