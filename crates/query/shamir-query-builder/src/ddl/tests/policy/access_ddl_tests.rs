//! Tests for Access tree, chmod / chown / chgrp, Group DDL, Auth users / roles,
//! and `res` helpers.

use shamir_query_types::batch::BatchOp;
use shamir_query_types::hmac;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::ddl;

use crate::ddl::tests::helpers::roundtrip;

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
    let op = ddl::chmod(ddl::res::table("mydb", "main", "users"), 0o700)
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chmod": {
                "table": ["mydb", "main", "users"]
            },
            "mode": 448,
            "hmac": "abc"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn chmod_with_hmac() {
    let op = ddl::chmod(ddl::res::table("mydb", "main", "users"), 0o700)
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chmod": {
                "table": ["mydb", "main", "users"]
            },
            "mode": 448,
            "hmac": "abc"
        })
    );
}

#[test]
fn chmod_function_namespace_wire() {
    let op = ddl::chmod(ddl::res::function_namespace(), 0o755)
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chmod": {
                "function_namespace": true
            },
            "mode": 493,
            "hmac": "abc"
        })
    );
}

#[test]
fn chown_database_wire() {
    let op = ddl::chown(ddl::res::database("testdb"), 7)
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chown": {
                "database": "testdb"
            },
            "owner": 7,
            "hmac": "abc"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn chown_with_hmac() {
    let op = ddl::chown(ddl::res::database("testdb"), 7)
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chown": {
                "database": "testdb"
            },
            "owner": 7,
            "hmac": "abc"
        })
    );
}

#[test]
fn chown_function_wire() {
    let op = ddl::chown(ddl::res::function("my_fn"), 10)
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chown": {
                "function": "my_fn"
            },
            "owner": 10,
            "hmac": "abc"
        })
    );
}

#[test]
fn chgrp_store_wire() {
    let op = ddl::chgrp(ddl::res::store("testdb", "main"), Some(3))
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chgrp": {
                "store": ["testdb", "main"]
            },
            "group": 3,
            "hmac": "abc"
        })
    );
}

#[test]
fn chgrp_with_hmac() {
    let op = ddl::chgrp(ddl::res::store("testdb", "main"), Some(3))
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chgrp": {
                "store": ["testdb", "main"]
            },
            "group": 3,
            "hmac": "abc"
        })
    );
}

// ============================================================================
// res::function_folder
// ============================================================================

#[test]
fn chmod_function_folder_wire() {
    // ResourceRef::FunctionFolder carries a Vec<String> of path segments.
    let op = ddl::chmod(ddl::res::function_folder(["reports", "daily"]), 0o755)
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chmod": {
                "function_folder": ["reports", "daily"]
            },
            "mode": 493,
            "hmac": "abc"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn chown_function_folder_wire() {
    let op = ddl::chown(ddl::res::function_folder(["reports"]), 7)
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chown": {
                "function_folder": ["reports"]
            },
            "owner": 7,
            "hmac": "abc"
        })
    );
}

#[test]
fn chgrp_null_group_wire() {
    let op = ddl::chgrp(ddl::res::database("testdb"), None)
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "chgrp": {
                "database": "testdb"
            },
            "group": null,
            "hmac": "abc"
        })
    );
}

// ============================================================================
// Group DDL
// ============================================================================

#[test]
fn create_group_wire() {
    let op = ddl::create_group("devs").hmac("abc").build().unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "create_group": "devs",
            "hmac": "abc"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn drop_group_by_name_wire() {
    let op = ddl::drop_group(ddl::GroupRef::Name {
        name: "devs".into(),
    })
    .hmac("abc")
    .build()
    .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_group": {
                "name": "devs"
            },
            "hmac": "abc"
        })
    );
}

#[test]
fn drop_group_by_id_wire() {
    let op = ddl::drop_group(ddl::GroupRef::Id { id: 3 })
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "drop_group": {
                "id": 3
            },
            "hmac": "abc"
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
    )
    .hmac("abc")
    .build()
    .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "add_group_member": {
                "name": "devs"
            },
            "user": 42,
            "hmac": "abc"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn remove_group_member_wire() {
    let op = ddl::remove_group_member(ddl::GroupRef::Id { id: 1 }, 42)
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "remove_group_member": {
                "id": 1
            },
            "user": 42,
            "hmac": "abc"
        })
    );
}

// ============================================================================
// Auth DDL (users + roles)
// ============================================================================

#[test]
fn rv_query_builder_fc88a91a_password_is_wrapped_at_builder_creation() {
    let builder = ddl::create_user("alice", "s3cret");
    assert!(format!("{builder:?}").contains("SecretString(***)"));
    drop(builder);
    let operation = ddl::create_user("alice", "s3cret").build();
    assert!(!format!("{operation:?}").contains("s3cret"));
}

#[test]
fn create_user_minimal() {
    let op = ddl::create_user("alice", "s3cret")
        .hmac("abc")
        .build()
        .unwrap();
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
        .database("testdb")
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(j["create_user"], "bob");
    assert_eq!(j["roles"], mpack!(["admin", "viewer"]));
    assert_eq!(j["database"], "testdb");
}

#[test]
fn drop_user_wire() {
    let op = ddl::drop_user("alice").hmac("abc").build().unwrap();
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
fn drop_user_with_hmac() {
    let op = ddl::drop_user("alice").hmac("abc").build().unwrap();
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
fn grant_role_wire() {
    let op = ddl::grant_role("admin", "alice")
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "grant_role": "admin",
            "user": "alice",
            "hmac": "abc"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn grant_role_with_hmac() {
    let op = ddl::grant_role("admin", "alice")
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "grant_role": "admin",
            "user": "alice",
            "hmac": "abc"
        })
    );
}

#[test]
fn revoke_role_wire() {
    let op = ddl::revoke_role("admin", "alice")
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "revoke_role": "admin",
            "user": "alice",
            "hmac": "abc"
        })
    );
}

#[test]
fn revoke_role_with_hmac() {
    let op = ddl::revoke_role("admin", "alice")
        .hmac("abc")
        .build()
        .unwrap();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        mpack!({
            "revoke_role": "admin",
            "user": "alice",
            "hmac": "abc"
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

// ============================================================================
// sign(): shared canonical v2 HMAC sealing for chmod / chown / chgrp
// ============================================================================

/// Extract the attached tag from any of the three signed access ops.
fn signed_tag(op: &BatchOp) -> String {
    match op {
        BatchOp::Chmod(o) => o.hmac.clone().expect("signed chmod"),
        BatchOp::Chown(o) => o.hmac.clone().expect("signed chown"),
        BatchOp::Chgrp(o) => o.hmac.clone().expect("signed chgrp"),
        _ => panic!("expected an access op"),
    }
}

#[test]
fn chmod_sign_tag_covers_payload_not_tag_field() {
    let key = [0x11u8; 32];
    let resource = ddl::res::table("mydb", "main", "a");
    let builder = ddl::chmod(resource.clone(), 0o640);
    let signed = builder.sign(&key, "mydb").unwrap();
    let tag = signed_tag(&signed);
    // The canonical v2 encoder ignores the `hmac` field, so a separately
    // built op carrying a deliberately wrong tag yields exactly the bytes
    // the server verifies.
    let wrong = ddl::chmod(resource.clone(), 0o640).hmac("0".repeat(64));
    let separate = wrong.build().unwrap();
    let canonical = hmac::canonical_batch_op("mydb", &separate).unwrap();
    assert_eq!(tag, hmac::compute_tag_hex(&key, &canonical));
    assert!(hmac::verify_tag_hex(&key, &canonical, &tag));
    // An altered resource or mode changes the tag.
    let other = ddl::chmod(ddl::res::table("mydb", "main", "b"), 0o640);
    assert_ne!(tag, signed_tag(&other.sign(&key, "mydb").unwrap()));
    let mode = ddl::chmod(resource.clone(), 0o600);
    assert_ne!(tag, signed_tag(&mode.sign(&key, "mydb").unwrap()));
    // The canonical v2 encoder for these three ops covers the resource (which
    // names the db) and the payload, not the routing `db` argument itself, so
    // a different `db` string yields the same tag — the tag comes from the
    // shared helper, not a local re-encoding.
    let db = ddl::chmod(resource, 0o640);
    assert_eq!(tag, signed_tag(&db.sign(&key, "otherdb").unwrap()));
    // A different session key does not verify.
    assert!(!hmac::verify_tag_hex(&[0x22u8; 32], &canonical, &tag));
}

#[test]
fn chown_sign_tag_covers_payload_not_tag_field() {
    let key = [0x33u8; 32];
    let resource = ddl::res::database("testdb");
    let builder = ddl::chown(resource.clone(), 7);
    let signed = builder.sign(&key, "testdb").unwrap();
    let tag = signed_tag(&signed);
    let wrong = ddl::chown(resource.clone(), 7).hmac("0".repeat(64));
    let separate = wrong.build().unwrap();
    let canonical = hmac::canonical_batch_op("testdb", &separate).unwrap();
    assert_eq!(tag, hmac::compute_tag_hex(&key, &canonical));
    assert!(hmac::verify_tag_hex(&key, &canonical, &tag));
    // An altered resource or owner changes the tag.
    let other = ddl::chown(ddl::res::function("f"), 7);
    assert_ne!(tag, signed_tag(&other.sign(&key, "testdb").unwrap()));
    let owner = ddl::chown(resource.clone(), 8);
    assert_ne!(tag, signed_tag(&owner.sign(&key, "testdb").unwrap()));
    // The canonical v2 encoder for these three ops covers the resource (which
    // names the db) and the payload, not the routing `db` argument itself, so
    // a different `db` string yields the same tag.
    let db = ddl::chown(resource, 7);
    assert_eq!(tag, signed_tag(&db.sign(&key, "otherdb").unwrap()));
    // A different session key does not verify.
    assert!(!hmac::verify_tag_hex(&[0x44u8; 32], &canonical, &tag));
}

#[test]
fn chgrp_sign_tag_covers_payload_not_tag_field() {
    let key = [0x55u8; 32];
    let resource = ddl::res::store("testdb", "main");
    let builder = ddl::chgrp(resource.clone(), Some(3));
    let signed = builder.sign(&key, "testdb").unwrap();
    let tag = signed_tag(&signed);
    let wrong = ddl::chgrp(resource.clone(), Some(3)).hmac("0".repeat(64));
    let separate = wrong.build().unwrap();
    let canonical = hmac::canonical_batch_op("testdb", &separate).unwrap();
    assert_eq!(tag, hmac::compute_tag_hex(&key, &canonical));
    assert!(hmac::verify_tag_hex(&key, &canonical, &tag));
    // An altered resource or group (including clearing it) changes the tag.
    let other = ddl::chgrp(ddl::res::store("testdb", "other"), Some(3));
    assert_ne!(tag, signed_tag(&other.sign(&key, "testdb").unwrap()));
    let group = ddl::chgrp(resource.clone(), Some(4));
    assert_ne!(tag, signed_tag(&group.sign(&key, "testdb").unwrap()));
    let cleared = ddl::chgrp(resource.clone(), None);
    assert_ne!(tag, signed_tag(&cleared.sign(&key, "testdb").unwrap()));
    // The canonical v2 encoder for these three ops covers the resource (which
    // names the db) and the payload, not the routing `db` argument itself, so
    // a different `db` string yields the same tag.
    let db = ddl::chgrp(resource, Some(3));
    assert_eq!(tag, signed_tag(&db.sign(&key, "otherdb").unwrap()));
    // A different session key does not verify.
    assert!(!hmac::verify_tag_hex(&[0x66u8; 32], &canonical, &tag));
}
