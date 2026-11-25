use crate::http_rest::common::{ping_body, request, Harness, ADMIN_PASSWORD};
use shamir_client::{Client, ClientError, ConnectOptions};
use shamir_connect::server::token::{TokenChannels, TokenMode};
use shamir_query_builder::{batch::Batch, ddl, token_admin};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::wire::DbRequest;
use std::time::Duration;
use zeroize::Zeroizing;

use super::privilege_cases::{admin_batch_cases, admin_request_cases};

fn classify_request(request: &DbRequest) -> &'static str {
    match request {
        DbRequest::Ping
        | DbRequest::WaitForVisibility { .. }
        | DbRequest::Execute { .. }
        | DbRequest::TxBegin { .. }
        | DbRequest::TxExecute { .. }
        | DbRequest::TxCommit { .. }
        | DbRequest::TxRollback { .. }
        | DbRequest::CreateCursor { .. }
        | DbRequest::FetchNext { .. }
        | DbRequest::CancelCursor { .. }
        | DbRequest::GetDdlOpStatus { .. } => "data",
        DbRequest::CreateScramUser { .. }
        | DbRequest::Repl(_)
        | DbRequest::ChangePasswordChallenge { .. }
        | DbRequest::ChangePasswordVerify(_)
        | DbRequest::SetSuperuser { .. }
        | DbRequest::SetReplicator { .. }
        | DbRequest::CreateApiToken { .. }
        | DbRequest::ListApiTokens { .. }
        | DbRequest::RevokeApiToken { .. }
        | DbRequest::RevokeUserTokens { .. }
        // R5 PR3: the operator state snapshot and the exact-key point read
        // are outside the token read-only subset (§3.3).
        | DbRequest::Status
        | DbRequest::GetRecord { .. } => "admin-denied",
    }
}

fn classify_batch(op: &BatchOp) -> &'static str {
    match op {
        BatchOp::Read(_)
        | BatchOp::Insert(_)
        | BatchOp::Update(_)
        | BatchOp::Set(_)
        | BatchOp::Delete(_)
        | BatchOp::Call(_)
        | BatchOp::Batch(_)
        | BatchOp::ForEach(_)
        | BatchOp::Subscribe(_)
        | BatchOp::Unsubscribe(_) => "data",
        BatchOp::List(_)
        | BatchOp::AccessTree(_)
        | BatchOp::DescribeTable(_)
        | BatchOp::GetTableSchema(_) => "metadata-exempt",
        BatchOp::CreateDb(_)
        | BatchOp::DropDb(_)
        | BatchOp::CreateRepo(_)
        | BatchOp::DropRepo(_)
        | BatchOp::RenameRepo(_)
        | BatchOp::AlterRepo(_)
        | BatchOp::RenameDb(_)
        | BatchOp::CreateTable(_)
        | BatchOp::DropTable(_)
        | BatchOp::RenameTable(_)
        | BatchOp::CreateIndex(_)
        | BatchOp::DropIndex(_)
        | BatchOp::RenameIndex(_)
        | BatchOp::SetBufferConfig(_)
        | BatchOp::GetBufferConfig(_)
        | BatchOp::AlterBufferConfig(_)
        | BatchOp::StartMigration(_)
        | BatchOp::CommitMigration(_)
        | BatchOp::RollbackMigration(_)
        | BatchOp::MigrationStatus(_)
        | BatchOp::CreateUser(_)
        | BatchOp::DropUser(_)
        | BatchOp::GrantRole(_)
        | BatchOp::RevokeRole(_)
        | BatchOp::Chmod(_)
        | BatchOp::Chown(_)
        | BatchOp::Chgrp(_)
        | BatchOp::CreateGroup(_)
        | BatchOp::DropGroup(_)
        | BatchOp::RenameGroup(_)
        | BatchOp::AddGroupMember(_)
        | BatchOp::RemoveGroupMember(_)
        | BatchOp::CreateFunction(_)
        | BatchOp::DropFunction(_)
        | BatchOp::RenameFunction(_)
        | BatchOp::DescribeFunction(_)
        | BatchOp::ExportFunction(_)
        | BatchOp::CheckFunction(_)
        | BatchOp::AlterFunction(_)
        | BatchOp::CreateValidator(_)
        | BatchOp::DropValidator(_)
        | BatchOp::RenameValidator(_)
        | BatchOp::BindValidator(_)
        | BatchOp::UnbindValidator(_)
        | BatchOp::ListValidators(_)
        | BatchOp::SetTableSchema(_)
        | BatchOp::AddSchemaRule(_)
        | BatchOp::RemoveSchemaRule(_)
        | BatchOp::DescribeRepo(_)
        | BatchOp::CreateFunctionFolder(_)
        | BatchOp::RenameFunctionFolder(_)
        | BatchOp::InternerDump(_)
        | BatchOp::InternerTouch(_)
        | BatchOp::PurgeHistory(_)
        | BatchOp::SetRetention(_)
        | BatchOp::ChangesSince(_)
        | BatchOp::CreateReplicationProfile(_)
        | BatchOp::DropReplicationProfile(_)
        | BatchOp::CreatePublication(_)
        | BatchOp::DropPublication(_)
        | BatchOp::CreateSubscription(_)
        | BatchOp::DropSubscription(_)
        | BatchOp::AlterSubscription(_)
        | BatchOp::ListPublications(_)
        | BatchOp::ListSubscriptions(_)
        // R1c: application-schema operations are admin ops for tokens.
        | BatchOp::ApplicationSchema(_)
        // R2a 3a: online-backup admin ops are superuser-surface for tokens.
        | BatchOp::StartBackup(_)
        | BatchOp::BackupStatus(_)
        // R4b-1: a token endpoint must not enumerate other sessions'
        // activity (Own) or global operator activity (All).
        | BatchOp::ListActivity(_)
        | BatchOp::ListShapeStats(_)
        | BatchOp::ResetShapeStats(_)
        | BatchOp::CancelRequest(_)
        // R4b-3: a token endpoint must not cancel another session's
        // transaction or cursor.
        | BatchOp::CancelTx(_)
        | BatchOp::CancelCursorAdmin(_)
        | BatchOp::CancelBackup(_)
        | BatchOp::ReplicationStatus(_)
        // R3 2b: online-export admin ops are superuser-surface for tokens.
        | BatchOp::StartExport(_)
        | BatchOp::ExportStatus(_)
        | BatchOp::CancelExport(_) => "admin-denied",
    }
}

async fn connect(harness: &Harness, username: &str, password: &[u8]) -> Client {
    tokio::time::timeout(
        Duration::from_secs(120),
        Client::connect(ConnectOptions {
            addr: harness.native_addr(),
            server_name: "localhost".into(),
            username: username.into(),
            password: Zeroizing::new(password.to_vec()),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: Some(Duration::from_secs(120)),
            handshake_timeout: Some(Duration::from_secs(120)),
            request_timeout: Some(Duration::from_secs(120)),
        }),
    )
    .await
    .expect("SCRAM connect timed out")
    .expect("SCRAM connect")
}

async fn shutdown(harness: Harness) {
    let report = tokio::time::timeout(Duration::from_secs(120), harness.handle.shutdown())
        .await
        .expect("server shutdown timed out");
    assert!(
        report.errors().is_empty(),
        "shutdown errors: {:?}",
        report.errors()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn elevated_owner_tokens_cannot_call_privileged_requests_or_batches() {
    let harness = Harness::launch_with_native_seed().await;
    let admin = connect(&harness, "admin", ADMIN_PASSWORD).await;
    let ordinary = admin
        .create_scram_user(
            "replicator-owner",
            Zeroizing::new("replicator-password".into()),
            vec![],
        )
        .await
        .expect("create replicator SCRAM account");
    assert_eq!(ordinary.len(), 16);
    admin
        .set_replicator("replicator-owner", true)
        .await
        .expect("grant replicator flag");
    let admin_created = admin
        .create_api_token(
            token_admin::create_api_token("privilege-superuser")
                .mode(shamir_query_types::wire::token_admin::ApiTokenMode::ReadWrite),
        )
        .await
        .expect("create admin-owned token");
    let replicator_created = admin
        .create_api_token(
            token_admin::create_api_token("privilege-replicator")
                .mode(shamir_query_types::wire::token_admin::ApiTokenMode::ReadWrite)
                .for_user("replicator-owner"),
        )
        .await
        .expect("create replicator-owned token");
    assert_eq!(admin_created.owner, "admin");
    assert_eq!(replicator_created.owner, "replicator-owner");
    let superuser_token = admin_created.token.reveal().to_owned();
    let replicator_token = replicator_created.token.reveal().to_owned();
    drop(admin);

    let cases = admin_request_cases();
    let batches = admin_batch_cases();
    assert!(!cases.is_empty(), "request fixtures are complete");
    assert_eq!(
        cases.len(),
        12,
        "every current admin-denied DbRequest variant has one fixture"
    );
    assert!(!batches.is_empty(), "batch fixtures are complete");
    assert_eq!(
        batches.len(),
        66,
        "one fixture for each current admin BatchOp arm"
    );

    let mut request_labels: Vec<&str> = cases.iter().map(|(_, label)| *label).collect();
    request_labels.sort_unstable();
    request_labels.dedup();
    assert_eq!(
        request_labels.len(),
        cases.len(),
        "request labels are unique"
    );
    let mut batch_labels: Vec<&str> = batches.iter().map(|(_, label)| *label).collect();
    batch_labels.sort_unstable();
    batch_labels.dedup();
    assert_eq!(batch_labels.len(), batches.len(), "batch labels are unique");

    for token in [&superuser_token, &replicator_token] {
        for (admin_request, label) in &cases {
            let classification = classify_request(admin_request);
            let body = rmp_serde::to_vec_named(admin_request).expect("encode typed request");
            let (status, _, response) = request(harness.addr(), Some(token), &body).await;
            assert_eq!(classification, "admin-denied", "{label}");
            assert_eq!(status, 403, "{label}");
            let response: shamir_query_types::wire::DbResponse =
                rmp_serde::from_slice(&response).expect("decode denial");
            assert!(
                matches!(response, shamir_query_types::wire::DbResponse::Error { code, .. } if code == "token_request_denied"),
                "{label}"
            );
        }
        for (op, label) in &batches {
            let classification = classify_batch(op);
            let mut batch = Batch::new();
            batch.op("admin", op.clone());
            let body = rmp_serde::to_vec_named(&DbRequest::Execute {
                query_version: shamir_query_types::wire::CURRENT_QUERY_LANG_VERSION,
                db: "default".into(),
                batch: batch.build(),
            })
            .expect("encode batch request");
            let (status, _, response) = request(harness.addr(), Some(token), &body).await;
            assert_eq!(classification, "admin-denied", "{label}");
            assert_eq!(status, 403, "{label}");
            let response: shamir_query_types::wire::DbResponse =
                rmp_serde::from_slice(&response).expect("decode denial");
            assert!(
                matches!(response, shamir_query_types::wire::DbResponse::Error { code, .. } if code == "token_request_denied"),
                "{label}"
            );
        }
        let (status, _, _) = request(harness.addr(), Some(token), &ping_body()).await;
        assert_eq!(
            status, 200,
            "Ping proves denials were not authentication failures"
        );
    }
    shutdown(harness).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metadata_scope_rules_are_observed_over_sockets() {
    let harness = Harness::launch().await;
    let scoped = harness
        .token_with_options(
            "metadata-scoped",
            TokenMode::ReadWrite,
            3600,
            TokenChannels::Both,
            Some(vec!["default".into()]),
        )
        .await
        .token
        .to_string();
    let unscoped = harness
        .token("metadata-unscoped", TokenMode::ReadWrite)
        .await;
    let cases = [
        (
            &scoped,
            shamir_query_builder::ddl::list_databases(),
            Some("token_database_denied"),
            "ListDatabases scoped",
        ),
        (
            &scoped,
            shamir_query_builder::ddl::list_users(),
            Some("token_database_denied"),
            "ListUsers scoped",
        ),
        (
            &scoped,
            shamir_query_builder::ddl::access_tree()
                .db("default")
                .build(),
            None,
            "AccessTree in scope",
        ),
        (
            &scoped,
            shamir_query_builder::ddl::access_tree()
                .db("outside")
                .build(),
            Some("token_database_denied"),
            "AccessTree outside scope",
        ),
        (
            &unscoped,
            shamir_query_builder::ddl::list_databases(),
            None,
            "ListDatabases unscoped",
        ),
        (
            &unscoped,
            shamir_query_builder::ddl::list_users(),
            None,
            "ListUsers unscoped",
        ),
    ];
    for (token, op, denial_code, label) in cases {
        let mut batch = Batch::new();
        batch.op("metadata", op);
        let body = rmp_serde::to_vec_named(&DbRequest::Execute {
            query_version: shamir_query_types::wire::CURRENT_QUERY_LANG_VERSION,
            db: "default".into(),
            batch: batch.build(),
        })
        .expect("encode metadata request");
        let (status, _, response) = request(harness.addr(), Some(token), &body).await;
        let response: shamir_query_types::wire::DbResponse =
            rmp_serde::from_slice(&response).expect("decode metadata response");
        if let Some(code) = denial_code {
            assert_eq!(status, 403, "{label}");
            assert!(
                matches!(response, shamir_query_types::wire::DbResponse::Error { code: actual, .. } if actual == code),
                "{label}"
            );
        } else {
            assert_eq!(status, 200, "{label}");
            assert!(
                !matches!(response, shamir_query_types::wire::DbResponse::Error { code, .. } if code.starts_with("token_")),
                "{label}"
            );
        }
    }
    // scope.rs denies List::{Databases,Users} whenever databases is Some despite
    // the Execute database being allowlisted; this contradicts the brief's allowance.
    shutdown(harness).await;
}

/// R3 step 2b, fail-closed online-export slice: `StartExport` /
/// `ExportStatus` / `CancelExport` are recognized by the db_handler even when
/// nested inside a `ForEach` body, and are refused BEFORE any database work.
/// Ordering: the SCRAM + superuser check precedes every form or
/// configuration detail, so a non-superuser session always gets
/// `permission_denied`; for the superuser the FORM check follows (shape
/// before root): a standalone single-op batch with a matching envelope is
/// dispatched only to find the fixture's `security.export_root` unset, so
/// it gets `online_export_not_configured`; a batch nested inside `ForEach`
/// is an invalid form and gets `online_export_invalid_form` — the shape is
/// refused before any root is consulted. Static messages, no artifact, no
/// dispatch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn online_export_ops_fail_closed_without_gate_or_root() {
    let harness = Harness::launch_with_native_seed().await;
    let admin = connect(&harness, "admin", ADMIN_PASSWORD).await;
    admin
        .create_scram_user(
            "plain-user",
            Zeroizing::new("plain-password".into()),
            vec![],
        )
        .await
        .expect("create plain SCRAM account");
    let plain = connect(&harness, "plain-user", b"plain-password").await;

    let cases: Vec<(&str, BatchOp)> = vec![
        ("start_export", ddl::start_export("default").build()),
        ("export_status", ddl::export_status()),
        ("cancel_export", ddl::cancel_export()),
    ];
    for (label, op) in &cases {
        let mut batch = Batch::new();
        batch.op("export", op.clone());
        let batch = batch.build();
        assert!(
            matches!(
                plain.execute("default", batch.clone()).await,
                Err(ClientError::Db { ref code, .. }) if code == "permission_denied"
            ),
            "{label}: non-superuser must get permission_denied"
        );
        assert!(
            matches!(
                admin.execute("default", batch.clone()).await,
                Err(ClientError::Db { ref code, .. }) if code == "online_export_not_configured"
            ),
            "{label}: superuser must get online_export_not_configured"
        );
    }

    // Nested mixed case: the export op hides inside a ForEach body; the
    // recognition must still fire (fail closed, no dispatch, no artifact).
    // The FORM refusal precedes the root check, so the superuser here gets
    // the shape code — the fixture's default `security.export_root` is
    // unset, and the shape check runs before any root would be consulted.
    let mut inner = Batch::new();
    inner.op("nested_export", ddl::start_export("default").build());
    let mut nested = Batch::new();
    nested.for_each("loop", vec!["row"], "row", inner.build());
    let nested = nested.build();
    assert!(
        matches!(
            plain.execute("default", nested.clone()).await,
            Err(ClientError::Db { ref code, .. }) if code == "permission_denied"
        ),
        "nested export inside ForEach: non-superuser must get permission_denied"
    );
    assert!(
        matches!(
            admin.execute("default", nested).await,
            Err(ClientError::Db { ref code, .. }) if code == "online_export_invalid_form"
        ),
        "nested export inside ForEach: superuser must get online_export_invalid_form \
         (shape before root)"
    );

    drop(plain);
    drop(admin);
    shutdown(harness).await;
}
