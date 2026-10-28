//! Nested batch read of a forbidden table stays denied.
use crate::common::*;
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::server::{BootstrapMode, ServerLauncher};
use tempfile::TempDir;
use zeroize::Zeroizing;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nested_batch_read_of_forbidden_table_stays_denied() {
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

    let db_name = "gate553e";
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
        &create_repo_table_req(db_name, secret_table),
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
    // secret_table is left at its enforced default (0o700, owner=System) —
    // carol below has no rights to it whatsoever.

    let carol_pw = b"carol-password".to_vec();
    let carol553e_roles: Vec<String> = vec![];
    let unsigned = DbRequest::CreateScramUser {
        name: "carol553e".into(),
        password: String::from_utf8(carol_pw.clone()).expect("utf8").into(),
        roles: carol553e_roles.clone(),
        hmac: None,
    };
    let carol553e_tag = shamir_query_types::hmac::compute_tag_hex(
        &shamir_query_types::hmac::derive_session_hmac_key(&admin_sid),
        &shamir_query_types::hmac::canonical_db_request(&unsigned)
            .expect("create-scram canonical form"),
    );
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "carol553e".into(),
            password: String::from_utf8(carol_pw.clone()).expect("utf8").into(),
            roles: carol553e_roles,
            hmac: Some(carol553e_tag),
        },
        admin_sid,
        &mut admin_rid,
        &mut admin_w,
        &mut admin_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::UserCreated { .. }),
        "create carol: {:?}",
        resp
    );

    let (carol_sid, mut carol_r, mut carol_w, mut carol_rid) =
        scram_login(server_addr, "carol553e", &carol_pw).await;

    // Carol sends Batch{ sub: Read(secret_table) }. The container is not an
    // admin op, so the coarse gate lets the data-only leaf through and the
    // per-resource authorization denies the READ on the forbidden table.
    let resp = roundtrip(
        &nested_batch_read_req(db_name, secret_table),
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
                "nested Batch{{Read(forbidden)}} must be denied by per-resource authorization, got code={}, msg={}",
                code, message
            );
        }
        other => panic!(
            "expected access_denied for nested Batch{{Read(forbidden)}}, got {:?}",
            other
        ),
    }

    cleanup(&mut carol_w, &mut carol_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}
