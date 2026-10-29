//! Scenarios 1-3: chmod deny/allow by mode, group grant, open default.
use super::common::*;
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::server::{BootstrapMode, ServerLauncher};
use tempfile::TempDir;
use zeroize::Zeroizing;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permission_deny_allow_by_mode() {
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

    // --- Admin login ---
    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    // --- Admin: create db + repo + table ---
    let db_name = "ptest";
    let table_name = "secret";

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

    // --- Admin: chmod table to 0o700 (owner-only rwx) ---
    let resp = roundtrip(
        &chmod_batch(admin_sid, db_name, "main", table_name, 0o700),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chmod: {:?}",
        resp
    );

    // --- Create a non-admin user ---
    let user_pw = b"user-password".to_vec();
    let create_alice_roles: Vec<String> = vec![];
    let unsigned = DbRequest::CreateScramUser {
        name: "alice".into(),
        password: String::from_utf8(user_pw.clone()).expect("utf8").into(),
        roles: create_alice_roles.clone(),
        hmac: None,
    };
    let create_alice_tag = shamir_query_types::hmac::compute_tag_hex(
        &shamir_query_types::hmac::derive_session_hmac_key(&admin_sid),
        &shamir_query_types::hmac::canonical_db_request(&unsigned)
            .expect("create-scram canonical form"),
    );
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "alice".into(),
            password: String::from_utf8(user_pw.clone()).expect("utf8").into(),
            roles: create_alice_roles,
            hmac: Some(create_alice_tag),
        },
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::UserCreated { .. }),
        "create user: {:?}",
        resp
    );

    // --- Alice logs in ---
    let (alice_sid, mut alice_r, mut alice_w, mut alice_rid) =
        scram_login(server_addr, "alice", &user_pw).await;

    // --- Alice tries to read from secret table → DENIED ---
    let resp = roundtrip(
        &read_req(db_name, table_name),
        alice_sid,
        &mut alice_rid,
        &mut alice_w,
        &mut alice_r,
    )
    .await;
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "access_denied",
                "alice should be denied, got code={}, msg={}",
                code, message
            );
            assert!(
                message.contains("access denied"),
                "message should describe denial: {}",
                message
            );
        }
        other => panic!("expected access_denied error, got {:?}", other),
    }

    // --- Admin reads from the same table → ALLOWED ---
    let resp = roundtrip(
        &read_req(db_name, table_name),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "admin should be allowed: {:?}",
        resp
    );

    // Cleanup
    cleanup(&mut alice_w, &mut alice_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}

/// Scenario 2: group grant — user allowed via group, non-member denied.
///
/// Three SCRAM connections from the same loopback subnet (admin + bob +
/// carol); the test config sets `auth_init_rate_per_second = 1000` so the
/// §8.6 warmup rate (/4 = 250/sec) does not throttle multi-login.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permission_group_grant() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = TempDir::new().expect("tempdir");
    let config = make_test_config(&temp);

    let admin_pw = b"admin-password".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_pw.clone()),
    };

    let launcher = ServerLauncher { config, bootstrap };
    let handle = match launcher.launch().await {
        Ok(h) => h,
        Err(e) => panic!("launcher boot failed: {e}"),
    };
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");
    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    let db_name = "gtest";
    let table_name = "shared";

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

    // G.4c: new objects default to enforced (0o700, owner=System since admin
    // is superuser). The group-grant test must let a group member traverse the
    // db + repo ancestors to reach the table (whose group bits are the SUBJECT
    // of this test). Open the db + repo so Execute is granted to everyone.
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

    // Create two non-admin users: bob (member) and carol (non-member).
    // Capture bob's directory-minted user_id bytes from the UserCreated
    // response — under the principal64 identity model bob's real principal
    // id is principal64(those bytes), NOT a hash of the string "bob".
    let bob_pw = b"bob-password".to_vec();
    let carol_pw = b"carol-password".to_vec();
    let mut bob_user_id: Option<Vec<u8>> = None;

    for (name, pw) in [("bob", &bob_pw), ("carol", &carol_pw)] {
        let loop_roles: Vec<String> = vec![];
        let unsigned = DbRequest::CreateScramUser {
            name: name.into(),
            password: String::from_utf8(pw.clone()).expect("utf8").into(),
            roles: loop_roles.clone(),
            hmac: None,
        };
        let loop_tag = shamir_query_types::hmac::compute_tag_hex(
            &shamir_query_types::hmac::derive_session_hmac_key(&admin_sid),
            &shamir_query_types::hmac::canonical_db_request(&unsigned)
                .expect("create-scram canonical form"),
        );
        let resp = roundtrip(
            &DbRequest::CreateScramUser {
                name: name.into(),
                password: String::from_utf8(pw.clone()).expect("utf8").into(),
                roles: loop_roles,
                hmac: Some(loop_tag),
            },
            admin_sid,
            &mut admin_rid,
            &mut admin_w,
            &mut admin_r,
        )
        .await;
        match resp {
            DbResponse::UserCreated { name: _, user_id } => {
                if name == "bob" {
                    bob_user_id = Some(user_id);
                }
            }
            other => panic!("create {name}: unexpected response {other:?}"),
        }
    }

    // --- Admin: create group ---
    let resp = roundtrip(
        &create_group_req(admin_sid, db_name, "devs"),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "create_group: {:?}",
        resp
    );

    // Extract the group_id from the response.
    let group_id: u64 = match &resp {
        DbResponse::Batch { response } => response
            .results
            .get("mk")
            .and_then(|r| r.records.first())
            .and_then(|r| r.get_value_u64("group_id"))
            .expect("group_id in response"),
        _ => panic!("unexpected"),
    };

    // Add bob to the group using bob's REAL principal64 id — projected from
    // the directory-minted user_id bytes returned by CreateScramUser, NOT a
    // hash of the username "bob" (which is what the old principal_id did).
    let bob_user_id_bytes: [u8; 16] = bob_user_id
        .expect("bob was created")
        .try_into()
        .expect("user_id is 16 bytes");
    let bob_pid = shamir_types::access::principal64(bob_user_id_bytes);
    let resp = roundtrip(
        &add_group_member_req(admin_sid, db_name, "devs", bob_pid),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "add_member: {:?}",
        resp
    );

    // chgrp the table to the group
    let resp = roundtrip(
        &chgrp_batch(admin_sid, db_name, "main", table_name, group_id),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chgrp: {:?}",
        resp
    );

    // chmod: owner rwx + group r-x + other --- (0o750)
    let resp = roundtrip(
        &chmod_batch(admin_sid, db_name, "main", table_name, 0o750),
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "chmod: {:?}",
        resp
    );

    // --- Bob (group member) reads → ALLOWED ---
    let (bob_sid, mut bob_r, mut bob_w, mut bob_rid) =
        scram_login(server_addr, "bob", &bob_pw).await;

    let resp = roundtrip(
        &read_req(db_name, table_name),
        bob_sid,
        &mut bob_rid,
        &mut bob_w,
        &mut bob_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "bob (group member) should be allowed: {:?}",
        resp
    );

    // --- Carol (non-member) reads → DENIED ---
    let (carol_sid, mut carol_r, mut carol_w, mut carol_rid) =
        scram_login(server_addr, "carol", &carol_pw).await;

    let resp = roundtrip(
        &read_req(db_name, table_name),
        carol_sid,
        &mut carol_rid,
        &mut carol_w,
        &mut carol_r,
    )
    .await;
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "access_denied",
                "carol should be denied, got code={}, msg={}",
                code, message
            );
        }
        other => panic!("expected access_denied for carol, got {:?}", other),
    }

    // Cleanup
    cleanup(&mut bob_w, &mut bob_r).await;
    cleanup(&mut carol_w, &mut carol_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}

/// Scenario 3: enforced default denies a stranger; explicit OPEN (0o777)
/// allows any authenticated user.
///
/// G.4c (Strategy A): new objects default to enforced owner-rwx (0o700), so a
/// non-owner stranger is now DENIED by default. This test proves BOTH paths:
///   (1) the enforced default denies a stranger (no chmod);
///   (2) after an explicit chmod to OPEN (0o777) on db + repo + table, any
///       authenticated user is ALLOWED (the open-mode path is unchanged).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permission_open_default_allows_any_user() {
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

    // --- Admin login ---
    let (admin_sid, mut admin_r, mut admin_w, mut admin_rid) =
        scram_login(server_addr, "admin", &admin_pw).await;

    let db_name = "opentest";
    let table_name = "public";

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

    // No chmod on the table yet — G.4c: create defaults are enforced (0o700).
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

    // Create a regular user
    let dave_pw = b"dave-password".to_vec();
    let dave_roles: Vec<String> = vec![];
    let unsigned = DbRequest::CreateScramUser {
        name: "dave".into(),
        password: String::from_utf8(dave_pw.clone()).expect("utf8").into(),
        roles: dave_roles.clone(),
        hmac: None,
    };
    let dave_tag = shamir_query_types::hmac::compute_tag_hex(
        &shamir_query_types::hmac::derive_session_hmac_key(&admin_sid),
        &shamir_query_types::hmac::canonical_db_request(&unsigned)
            .expect("create-scram canonical form"),
    );
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "dave".into(),
            password: String::from_utf8(dave_pw.clone()).expect("utf8").into(),
            roles: dave_roles,
            hmac: Some(dave_tag),
        },
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::UserCreated { .. }),
        "create dave: {:?}",
        resp
    );

    // --- Dave reads from public table → DENIED (enforced default 0o700) ---
    let (dave_sid, mut dave_r, mut dave_w, mut dave_rid) =
        scram_login(server_addr, "dave", &dave_pw).await;

    let resp = roundtrip(
        &read_req(db_name, table_name),
        dave_sid,
        &mut dave_rid,
        &mut dave_w,
        &mut dave_r,
    )
    .await;
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(
            code, "access_denied",
            "enforced default (0o700) should deny a non-owner stranger"
        ),
        other => panic!(
            "expected access_denied under enforced default, got {:?}",
            other
        ),
    }

    // --- Admin chmod db + repo + table to OPEN (0o777) ---
    for req in [
        chmod_db_batch(admin_sid, db_name, 0o777),
        chmod_store_batch(admin_sid, db_name, "main", 0o777),
        chmod_batch(admin_sid, db_name, "main", table_name, 0o777),
    ] {
        let resp = roundtrip(&req, admin_sid, &mut admin_rid, &mut admin_w, &mut admin_r).await;
        assert!(
            matches!(resp, DbResponse::Batch { .. }),
            "chmod open: {:?}",
            resp
        );
    }

    // --- Dave reads again → ALLOWED (explicit OPEN path is unchanged) ---
    let resp = roundtrip(
        &read_req(db_name, table_name),
        dave_sid,
        &mut dave_rid,
        &mut dave_w,
        &mut dave_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "dave should be allowed after explicit chmod 0o777: {:?}",
        resp
    );

    // Cleanup
    cleanup(&mut dave_w, &mut dave_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}

// --------------------------------------------------------------------------
// Task #553: wire-admin coarse DAC gate — explicit 4-op allowlist
// --------------------------------------------------------------------------
//
// Covers the test matrix from
// `docs/dev-artifacts/prompts/audit/71-wire-admin-gate-explicit-allowlist.md`:
//   (a) non-superuser CAN DescribeTable/GetTableSchema a table they can Read
//   (b) non-superuser CANNOT DescribeTable/GetTableSchema a table with no rights
//   (c) AccessTree / List(Users) / List(Roles) stay DENIED for a non-superuser
//   (d) List(Databases) is ALLOWED for a non-superuser
//   (e) a nested Batch{ Read(forbidden_table) } is STILL DENIED (Batch is
//       never in the allowlist — the regression test for the rejected
//       is_write()-based relaxation)
//   (f) every other non-exempted is_admin() op remains superuser-only,
//       including the 8 ops the rejected approach would have silently
//       exempted via is_write() == false.
