//! Coarse-grained gate: describe/get_table_schema/access-tree/list
//! semantics and non-exempted superuser-only ops.
use super::common::*;
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::server::{BootstrapMode, ServerLauncher};
use tempfile::TempDir;
use zeroize::Zeroizing;

/// (a) + (b): DescribeTable/GetTableSchema pass the coarse gate for a
/// non-superuser, but the op's OWN per-table authorization still applies
/// underneath — allowed on a table the actor can Read, denied on one they
/// cannot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn coarse_gate_describe_and_get_schema_follow_own_table_authz() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    let admin_pw = b"admin-password".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_pw.clone()),
    };

    let launcher = ServerLauncher { config, bootstrap };
    let handle = launcher.launch().await.expect("launcher boot");
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");

    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    let db_name = "gate553a";
    let readable_table = "readable";
    let secret_table = "secret";

    let resp = roundtrip(
        &create_db_req(db_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "create_db: {:?}",
        resp
    );

    let resp = roundtrip(
        &create_repo_table_req(db_name, readable_table),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "setup {}: {:?}",
        readable_table,
        resp
    );

    let resp = roundtrip(
        &create_table_only_req(db_name, secret_table),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "setup {}: {:?}",
        secret_table,
        resp
    );

    // Open the database + store ancestors (a *separate* pre-check from the
    // coarse admin gate under test: `execute_as` requires `Action::Read` on
    // the `db` named in the request envelope before dispatching ANY batch,
    // and per-table authorization requires `Action::Execute` traversal on
    // the store ancestor, G.4c). Without this, alice's requests below would
    // fail at these earlier gates, not at the per-table check this test
    // targets.
    let resp = roundtrip(
        &chmod_db_batch(admin_sid, db_name, 0o755),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chmod_db: {:?}",
        resp
    );

    let resp = roundtrip(
        &chmod_store_batch(admin_sid, db_name, "main", 0o755),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chmod_store: {:?}",
        resp
    );

    // Open `readable_table` to any authenticated user; leave `secret_table`
    // at its enforced default (0o700, owner=System) so alice has NO rights.
    let resp = roundtrip(
        &chmod_batch(admin_sid, db_name, "main", readable_table, 0o777),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chmod open: {:?}",
        resp
    );

    let alice_pw = b"alice-password".to_vec();
    let alice_roles2: Vec<String> = vec![];
    let unsigned = DbRequest::CreateScramUser {
        name: "alice".into(),
        password: String::from_utf8(alice_pw.clone()).expect("utf8").into(),
        roles: alice_roles2.clone(),
        hmac: None,
    };
    let alice_tag2 = shamir_query_types::hmac::compute_tag_hex(
        &shamir_query_types::hmac::derive_session_hmac_key(&admin_sid),
        &shamir_query_types::hmac::canonical_db_request(&unsigned)
            .expect("create-scram canonical form"),
    );
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "alice".into(),
            password: String::from_utf8(alice_pw.clone()).expect("utf8").into(),
            roles: alice_roles2,
            hmac: Some(alice_tag2),
        },
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::UserCreated { .. }),
        "create alice: {:?}",
        resp
    );

    let (alice_sid, mut alice_r, mut alice_w, mut alice_rid) =
        scram_login(server_addr, "alice", &alice_pw).await;

    // (a) DescribeTable/GetTableSchema on the readable table: coarse gate
    // passes (allowlisted) AND the per-table Read check passes → allowed.
    let resp = roundtrip(
        &describe_table_req(db_name, readable_table),
        alice_sid,
        &mut alice_rid,
        &mut alice_w,
        &mut alice_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "alice DescribeTable(readable) should be allowed: {:?}",
        resp
    );

    let resp = roundtrip(
        &get_table_schema_req(db_name, readable_table),
        alice_sid,
        &mut alice_rid,
        &mut alice_w,
        &mut alice_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "alice GetTableSchema(readable) should be allowed: {:?}",
        resp
    );

    // (b) DescribeTable/GetTableSchema on the secret table: coarse gate
    // passes it through (allowlisted), but the op's own per-table Read
    // check denies — proves the coarse-gate relaxation does not itself
    // grant access.
    let resp = roundtrip(
        &describe_table_req(db_name, secret_table),
        alice_sid,
        &mut alice_rid,
        &mut alice_w,
        &mut alice_r,
    )
    .await;
    match resp {
        DbResponse::Error { code, .. } => {
            assert_eq!(
                code, "access_denied",
                "DescribeTable(secret) should be access_denied"
            );
        }
        other => panic!(
            "expected access_denied for DescribeTable(secret), got {:?}",
            other
        ),
    }

    let resp = roundtrip(
        &get_table_schema_req(db_name, secret_table),
        alice_sid,
        &mut alice_rid,
        &mut alice_w,
        &mut alice_r,
    )
    .await;
    match resp {
        DbResponse::Error { code, .. } => {
            assert_eq!(
                code, "access_denied",
                "GetTableSchema(secret) should be access_denied"
            );
        }
        other => panic!(
            "expected access_denied for GetTableSchema(secret), got {:?}",
            other
        ),
    }

    cleanup(&mut alice_w, &mut alice_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}

/// (c) + (d): `AccessTree` and `List(Users)`/`List(Roles)` stay denied for
/// a non-superuser (their own `Manage(Root)` gate still applies underneath
/// the now-passing coarse gate); `List(Databases)` is now ALSO denied by
/// default — task #615/#620 narrowed Root's default mode `0o755` -> `0o751`,
/// dropping Other-Read (so `List`, i.e. database-name enumeration, is
/// closed by default), while keeping Other-Execute (so ancestor-traversal
/// into resources an actor separately holds rights on is unaffected).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn coarse_gate_access_tree_and_list_semantics() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    let admin_pw = b"admin-password".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_pw.clone()),
    };

    let launcher = ServerLauncher { config, bootstrap };
    let handle = launcher.launch().await.expect("launcher boot");
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");

    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    let db_name = "gate553c";
    let resp = roundtrip(
        &create_db_req(db_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "create_db: {:?}",
        resp
    );

    // Open the dispatch-target database itself (a *separate* pre-check from
    // the coarse admin gate under test: `execute_as` requires `Action::Read`
    // on the `db` named in the request envelope before dispatching ANY
    // batch — irrespective of admin-op allowlisting). Without this, every
    // request below would fail at that earlier gate, not at the one this
    // test is targeting.
    let resp = roundtrip(
        &chmod_db_batch(admin_sid, db_name, 0o755),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chmod_db: {:?}",
        resp
    );

    let bob_pw = b"bob-password".to_vec();
    let bob553c_roles: Vec<String> = vec![];
    let unsigned = DbRequest::CreateScramUser {
        name: "bob553c".into(),
        password: String::from_utf8(bob_pw.clone()).expect("utf8").into(),
        roles: bob553c_roles.clone(),
        hmac: None,
    };
    let bob553c_tag = shamir_query_types::hmac::compute_tag_hex(
        &shamir_query_types::hmac::derive_session_hmac_key(&admin_sid),
        &shamir_query_types::hmac::canonical_db_request(&unsigned)
            .expect("create-scram canonical form"),
    );
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "bob553c".into(),
            password: String::from_utf8(bob_pw.clone()).expect("utf8").into(),
            roles: bob553c_roles,
            hmac: Some(bob553c_tag),
        },
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::UserCreated { .. }),
        "create bob: {:?}",
        resp
    );

    let (bob_sid, mut bob_r, mut bob_w, mut bob_rid) =
        scram_login(server_addr, "bob553c", &bob_pw).await;

    // (c) AccessTree — coarse gate now passes it through (allowlisted),
    // but `handle_access_tree`'s own `Manage(Root)` gate denies a
    // non-superuser. Expect `access_denied`, NOT `permission_denied`
    // (proves the coarse gate stopped blocking it outright).
    let resp = roundtrip(
        &access_tree_req(db_name),
        bob_sid,
        &mut bob_rid,
        &mut bob_w,
        &mut bob_r,
    )
    .await;
    match resp {
        DbResponse::Error { code, .. } => {
            assert_eq!(
                code, "access_denied",
                "AccessTree should be access_denied, not permission_denied"
            );
        }
        other => panic!("expected access_denied for AccessTree, got {:?}", other),
    }

    // (c) List(Users) — same reasoning: Manage(Root) denies a non-superuser.
    let resp = roundtrip(
        &list_users_req(db_name),
        bob_sid,
        &mut bob_rid,
        &mut bob_w,
        &mut bob_r,
    )
    .await;
    match resp {
        DbResponse::Error { code, .. } => {
            assert_eq!(code, "access_denied", "List(Users) should be access_denied");
        }
        other => panic!("expected access_denied for List(Users), got {:?}", other),
    }

    // (d) List(Databases) — task #615/#620: Root's default mode no longer
    // grants Other-Read, so database-name enumeration is DENIED by default
    // for a non-superuser (was ALLOWED before this task).
    let resp = roundtrip(
        &list_databases_req(db_name),
        bob_sid,
        &mut bob_rid,
        &mut bob_w,
        &mut bob_r,
    )
    .await;
    match resp {
        DbResponse::Error { code, .. } => {
            assert_eq!(
                code, "access_denied",
                "List(Databases) should be access_denied by default (task #615/#620)"
            );
        }
        other => panic!(
            "expected access_denied for List(Databases), got {:?}",
            other
        ),
    }

    cleanup(&mut bob_w, &mut bob_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}

/// (e) THE regression test: a nested `Batch{ Read(forbidden_table) }` must
/// stay DENIED for a non-superuser. A container carries no policy of its own:
/// the coarse admin gate classifies the LEAVES (`BatchTreeVisitor`), so the
/// data-only nested `Read` passes it and is then authorized per resource.
/// `collect_required_access` walks the whole tree, so the forbidden table is
/// refused with the structured `access_denied` instead of executing.
/// (f) Every OTHER non-exempted `is_admin()` op remains superuser-only:
/// `CreateDb`, `Chmod`, `CreateUser` (representative DML/DDL sample) plus
/// explicitly the 8 ops the REJECTED `is_write()`-based relaxation would
/// have silently exempted (`GetBufferConfig`, `MigrationStatus`,
/// `InternerDump`, `ChangesSince`, `ListValidators`, `ListPublications`,
/// `ListSubscriptions`, `ReplicationStatus`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn coarse_gate_non_exempted_ops_stay_superuser_only() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    let admin_pw = b"admin-password".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_pw.clone()),
    };

    let launcher = ServerLauncher { config, bootstrap };
    let handle = launcher.launch().await.expect("launcher boot");
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");

    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    let db_name = "gate553f";
    let table_name = "sometable";

    let resp = roundtrip(
        &create_db_req(db_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "create_db: {:?}",
        resp
    );

    let resp = roundtrip(
        &create_repo_table_req(db_name, table_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "setup: {:?}",
        resp
    );

    // Open the table so any per-op *table* authorization (irrelevant to
    // most ops below, but harmless) would not itself be the blocker —
    // isolates the assertion to the coarse gate.
    let resp = roundtrip(
        &chmod_batch(admin_sid, db_name, "main", table_name, 0o777),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chmod open: {:?}",
        resp
    );

    let eve_pw = b"eve-password".to_vec();
    let eve553f_roles: Vec<String> = vec![];
    let unsigned = DbRequest::CreateScramUser {
        name: "eve553f".into(),
        password: String::from_utf8(eve_pw.clone()).expect("utf8").into(),
        roles: eve553f_roles.clone(),
        hmac: None,
    };
    let eve553f_tag = shamir_query_types::hmac::compute_tag_hex(
        &shamir_query_types::hmac::derive_session_hmac_key(&admin_sid),
        &shamir_query_types::hmac::canonical_db_request(&unsigned)
            .expect("create-scram canonical form"),
    );
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "eve553f".into(),
            password: String::from_utf8(eve_pw.clone()).expect("utf8").into(),
            roles: eve553f_roles,
            hmac: Some(eve553f_tag),
        },
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::UserCreated { .. }),
        "create eve: {:?}",
        resp
    );

    let (eve_sid, mut eve_r, mut eve_w, mut eve_rid) =
        scram_login(server_addr, "eve553f", &eve_pw).await;

    let cases: Vec<(&str, DbRequest)> = vec![
        ("CreateDb", create_db_only_req(db_name, "gate553f_child")),
        ("Chmod", chmod_only_req(db_name, "main", table_name)),
        (
            "CreateUser",
            create_user_only_req(db_name, "gate553f_newuser"),
        ),
        (
            "GetBufferConfig",
            get_buffer_config_only_req(db_name, table_name),
        ),
        ("MigrationStatus", migration_status_only_req(db_name)),
        ("InternerDump", interner_dump_only_req(db_name)),
        ("ChangesSince", changes_since_only_req(db_name)),
        (
            "ListValidators",
            list_validators_only_req(db_name, table_name),
        ),
        ("ListPublications", list_publications_only_req(db_name)),
        ("ListSubscriptions", list_subscriptions_only_req(db_name)),
        ("ReplicationStatus", replication_status_only_req(db_name)),
    ];

    for (label, req) in cases {
        let resp = roundtrip(&req, eve_sid, &mut eve_rid, &mut eve_w, &mut eve_r).await;
        match resp {
            DbResponse::Error { code, message } => {
                assert_eq!(
                    code, "permission_denied",
                    "{} should be permission_denied for a non-superuser, got code={}, msg={}",
                    label, code, message
                );
            }
            other => panic!(
                "{} should remain superuser-only (permission_denied), got {:?}",
                label, other
            ),
        }
    }

    cleanup(&mut eve_w, &mut eve_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}
