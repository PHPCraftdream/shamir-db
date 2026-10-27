//! Shared fixtures, wire-frame mirrors, SCRAM login helper and request
//! builders for the `permission_e2e` integration tests.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::io::{split, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::{
    extract_tls_exporter, make_client_config_app_authenticated, AppLayerServerAuth,
};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, SecurityConfig,
    TlsConfig,
};
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::version::CURRENT_QUERY_LANG_VERSION;

// --------------------------------------------------------------------------
// Wire-frame mirrors (same as mvp_e2e.rs)
// --------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub(crate) struct WireAuthInit {
    user: String,
    #[serde(with = "serde_bytes")]
    client_nonce: Vec<u8>,
    binding_mode: u8,
    version: u8,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WireChallenge {
    #[serde(with = "serde_bytes")]
    salt: Vec<u8>,
    memory_kb: u32,
    time: u32,
    parallelism: u32,
    argon2_version: u8,
    #[serde(with = "serde_bytes")]
    server_nonce: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WireClientProof {
    #[serde(with = "serde_bytes")]
    client_proof: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WireAuthOk {
    #[serde(with = "serde_bytes")]
    server_signature: Vec<u8>,
    #[serde(with = "serde_bytes")]
    server_pub_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    identity_sig: Vec<u8>,
    #[serde(with = "serde_bytes")]
    session_id: Vec<u8>,
    expires_at_ns: u64,
    #[serde(default, with = "serde_bytes")]
    resumption_ticket: Vec<u8>,
    #[serde(default)]
    resumption_expires_at_ns: u64,
    #[serde(default)]
    server_query_version: u8,
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

pub(crate) fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

pub(crate) fn make_test_config(temp: &TempDir) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
    Config {
        durability: Default::default(),
        transactions: Default::default(),
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        kdf_defaults: fast_kdf(),
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: "127.0.0.1:0".to_string(),
            profile: ProfileKind::TlsExporter,
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: SecurityConfig {
            auth_init_rate_per_second: 1000,
            ..Default::default()
        },
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

pub(crate) async fn roundtrip<R, W>(
    req: &DbRequest,
    sid: [u8; 32],
    next_rid: &mut u32,
    w: &mut W,
    r: &mut R,
) -> DbResponse
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let bytes = rmp_serde::to_vec_named(req).expect("encode req");
    let rid = *next_rid;
    *next_rid += 1;
    let envelope = RequestEnvelope::new(sid, Some(rid), bytes);
    let envelope_bytes = envelope.to_msgpack().expect("envelope encode");
    write_frame(w, &envelope_bytes).await.expect("send request");
    let resp_bytes = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read_frame(r, MAX_FRAME_SIZE_DEFAULT),
    )
    .await
    .expect("response within 10s")
    .expect("read response");
    let resp_envelope = ResponseEnvelope::from_msgpack(&resp_bytes).expect("response envelope");
    assert_eq!(resp_envelope.request_id, Some(rid), "request_id echoed");
    rmp_serde::from_slice(&resp_envelope.res).expect("decode DbResponse")
}

/// SCRAM-authenticate and return (session_id, read_half, write_half, next_rid).
pub(crate) async fn scram_login(
    server_addr: std::net::SocketAddr,
    username: &str,
    password: &[u8],
) -> (
    [u8; 32],
    tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>,
    u32,
) {
    let norm_username = NormalizedUsername::from_raw(username).expect("username");

    let client_cfg =
        make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound()).unwrap();
    let connector = TlsConnector::from(client_cfg);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = match TcpStream::connect(server_addr).await {
        Ok(t) => t,
        Err(e) => panic!("TCP connect to {} failed: {}", server_addr, e),
    };
    let tls = match connector.connect(server_name, tcp).await {
        Ok(t) => t,
        Err(e) => panic!("TLS handshake to {} failed: {}", server_addr, e),
    };
    let exporter = extract_tls_exporter(&tls).expect("client exporter");

    let hs = HandshakeBuilder::new(norm_username, TransportKind::Tcp, BindingMode::TlsExporter)
        .tls_exporter(exporter)
        .accept_new_host(true)
        .build()
        .expect("handshake builder");

    let (mut r, mut w) = split(tls);

    // auth_init
    let init = hs.auth_init();
    let init_wire = WireAuthInit {
        user: init.user,
        client_nonce: init.client_nonce.to_vec(),
        binding_mode: init.binding_mode,
        version: init.version,
    };
    write_frame(&mut w, &rmp_serde::to_vec(&init_wire).unwrap())
        .await
        .expect("send auth_init");

    // challenge
    let ch_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT)
        .await
        .expect("read challenge");
    let ch_wire: WireChallenge = rmp_serde::from_slice(&ch_bytes).unwrap();
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&ch_wire.salt);
    let mut server_nonce = [0u8; 32];
    server_nonce.copy_from_slice(&ch_wire.server_nonce);
    let challenge = ServerChallenge {
        salt,
        kdf_params: KdfParams {
            memory_kb: ch_wire.memory_kb,
            time: ch_wire.time,
            parallelism: ch_wire.parallelism,
            argon2_version: ch_wire.argon2_version,
        },
        server_nonce,
    };

    // derive proof
    let mut password_buf = password.to_vec();
    let (proof, derived, am) = hs
        .process_challenge(&challenge, &mut password_buf)
        .expect("process challenge");

    // send proof
    let proof_wire = WireClientProof {
        client_proof: proof.to_vec(),
    };
    write_frame(&mut w, &rmp_serde::to_vec(&proof_wire).unwrap())
        .await
        .expect("send proof");

    // auth_ok
    let ok_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT)
        .await
        .expect("read auth_ok");
    let ok_wire: WireAuthOk = rmp_serde::from_slice(&ok_bytes).unwrap();

    let mut sig32 = [0u8; 32];
    sig32.copy_from_slice(&ok_wire.server_signature);
    let mut pub32 = [0u8; 32];
    pub32.copy_from_slice(&ok_wire.server_pub_key);
    let mut id_sig = [0u8; 64];
    id_sig.copy_from_slice(&ok_wire.identity_sig);
    let mut session_id = [0u8; 32];
    session_id.copy_from_slice(&ok_wire.session_id);

    let auth_ok = ServerAuthOk {
        server_signature: sig32,
        server_pub_key: pub32,
        identity_sig: id_sig,
        session_id,
        expires_at_ns: ok_wire.expires_at_ns,
        resumption_ticket: Some(ok_wire.resumption_ticket.clone()),
        resumption_expires_at_ns: Some(ok_wire.resumption_expires_at_ns),
        rotation_in_progress: None,
        kdf_upgrade_required: None,
    };

    hs.process_auth_ok(&auth_ok, &derived, &am, |_pin| {})
        .expect("process auth_ok");

    (session_id, r, w, 1)
}

/// Build a simple `BatchRequest` that reads from a table.
pub(crate) fn read_batch(table: &str) -> shamir_db::query::batch::BatchRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("read");
    b.query("rd", shamir_query_builder::Query::from(table));
    b.build()
}

/// Build a batch with a single chmod op on a table. `session_id` is the
/// signing session's bearer token — chmod is HMAC-gated (task #542).
pub(crate) fn chmod_batch(
    session_id: [u8; 32],
    db: &str,
    store: &str,
    table: &str,
    mode: u16,
) -> DbRequest {
    let resource = shamir_query_builder::ddl::res::table(db, store, table);
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let op = shamir_query_types::batch::BatchOp::Chmod(shamir_query_types::admin::ChmodOp {
        chmod: resource.clone(),
        mode,
        hmac: None,
    });
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_batch_op(db, &op).expect("chmod canonical form"),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chmod");
    b.chmod(
        "cm",
        shamir_query_builder::ddl::chmod(resource, mode)
            .hmac(tag)
            .build()
            .expect("build chmod"),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch with a single chmod op on a database (top-level container).
/// `session_id` is the signing session's bearer token.
pub(crate) fn chmod_db_batch(session_id: [u8; 32], db: &str, mode: u16) -> DbRequest {
    let resource = shamir_query_builder::ddl::res::database(db);
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let op = shamir_query_types::batch::BatchOp::Chmod(shamir_query_types::admin::ChmodOp {
        chmod: resource.clone(),
        mode,
        hmac: None,
    });
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_batch_op(db, &op).expect("chmod canonical form"),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chmod_db");
    b.chmod(
        "cm",
        shamir_query_builder::ddl::chmod(resource, mode)
            .hmac(tag)
            .build()
            .expect("build chmod"),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch with a single chmod op on a store/repo. `session_id` is
/// the signing session's bearer token.
pub(crate) fn chmod_store_batch(
    session_id: [u8; 32],
    db: &str,
    store: &str,
    mode: u16,
) -> DbRequest {
    let resource = shamir_query_builder::ddl::res::store(db, store);
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let op = shamir_query_types::batch::BatchOp::Chmod(shamir_query_types::admin::ChmodOp {
        chmod: resource.clone(),
        mode,
        hmac: None,
    });
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_batch_op(db, &op).expect("chmod canonical form"),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chmod_store");
    b.chmod(
        "cm",
        shamir_query_builder::ddl::chmod(resource, mode)
            .hmac(tag)
            .build()
            .expect("build chmod"),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch with a single chgrp op on a table. `session_id` is the
/// signing session's bearer token — chgrp is HMAC-gated (task #542).
pub(crate) fn chgrp_batch(
    session_id: [u8; 32],
    db: &str,
    store: &str,
    table: &str,
    group: u64,
) -> DbRequest {
    let resource = shamir_query_builder::ddl::res::table(db, store, table);
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let op = shamir_query_types::batch::BatchOp::Chgrp(shamir_query_types::admin::ChgrpOp {
        chgrp: resource.clone(),
        group: Some(group),
        hmac: None,
    });
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_batch_op(db, &op).expect("chgrp canonical form"),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chgrp");
    b.chgrp(
        "cg",
        shamir_query_builder::ddl::chgrp(resource, Some(group))
            .hmac(tag)
            .build()
            .expect("build chgrp"),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch creating a group. `session_id` is the signing session's
/// bearer token — create_group is HMAC-gated (task #551).
pub(crate) fn create_group_req(session_id: [u8; 32], db: &str, name: &str) -> DbRequest {
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let op =
        shamir_query_types::batch::BatchOp::CreateGroup(shamir_query_types::admin::CreateGroupOp {
            create_group: name.into(),
            hmac: None,
        });
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_batch_op(db, &op)
            .expect("create-group canonical form"),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("mkgrp");
    b.create_group(
        "mk",
        shamir_query_builder::ddl::create_group(name)
            .hmac(tag)
            .build()
            .expect("build create group"),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch adding a user to a group. `session_id` is the signing
/// session's bearer token — add_group_member is HMAC-gated (task #551).
pub(crate) fn add_group_member_req(
    session_id: [u8; 32],
    db: &str,
    group_name: &str,
    user_id: u64,
) -> DbRequest {
    let group = shamir_query_builder::ddl::GroupRef::Name {
        name: group_name.to_string(),
    };
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let op = shamir_query_types::batch::BatchOp::AddGroupMember(
        shamir_query_types::admin::AddGroupMemberOp {
            add_group_member: group.clone(),
            user: user_id,
            hmac: None,
        },
    );
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_batch_op(db, &op)
            .expect("add-group-member canonical form"),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("addmember");
    b.add_group_member(
        "am",
        shamir_query_builder::ddl::add_group_member(group, user_id)
            .hmac(tag)
            .build()
            .expect("build add group member"),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a create_db request.
pub(crate) fn create_db_req(db_name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("mkdb");
    b.create_db("mk", shamir_query_builder::ddl::create_db(db_name));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "default".into(),
        batch: b.build(),
    }
}

/// Build a create_repo + create_table request.
pub(crate) fn create_repo_table_req(db_name: &str, table_name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("setup");
    b.create_repo("mr", shamir_query_builder::ddl::create_repo("main"));
    b.create_table("tb", shamir_query_builder::ddl::create_table(table_name));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a create_table-only request (assumes `main` repo already exists).
pub(crate) fn create_table_only_req(db_name: &str, table_name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("setup2");
    b.create_table("tb2", shamir_query_builder::ddl::create_table(table_name));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a read request against a table.
pub(crate) fn read_req(db_name: &str, table_name: &str) -> DbRequest {
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: read_batch(table_name),
    }
}

/// Build a single-`DescribeTable`-op `DbRequest::Execute` (task #553
/// coarse-gate allowlist tests).
pub(crate) fn describe_table_req(db_name: &str, table: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("describe");
    b.describe_table("dt", shamir_query_builder::ddl::describe_table(table));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a single-`GetTableSchema`-op `DbRequest::Execute`.
pub(crate) fn get_table_schema_req(db_name: &str, table: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("getschema");
    b.get_table_schema("gs", shamir_query_builder::ddl::get_table_schema(table));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a single-`AccessTree`-op `DbRequest::Execute`.
pub(crate) fn access_tree_req(db_name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("tree");
    b.access_tree("at", shamir_query_builder::ddl::access_tree());
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a single-`List(Databases)`-op `DbRequest::Execute`.
pub(crate) fn list_databases_req(db_name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("lsdb");
    b.list_databases("ld", shamir_query_builder::ddl::list_databases());
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a single-`List(Users)`-op `DbRequest::Execute`.
pub(crate) fn list_users_req(db_name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("lsusers");
    b.list_users("lu", shamir_query_builder::ddl::list_users());
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a request with a single nested `Batch{ inner: Read(table) }` —
/// the direct regression test for the rejected `is_write()`-based
/// relaxation (task #553): `Batch` must stay OUT of the coarse-gate
/// allowlist, so a forbidden-table `Read` nested inside a sub-batch must
/// still be denied outright by the coarse gate itself.
pub(crate) fn nested_batch_read_req(db_name: &str, table: &str) -> DbRequest {
    let mut outer = shamir_query_builder::batch::Batch::new();
    outer.id("outer");
    let inner = read_batch(table);
    outer.sub_batch_no_bind("sub", inner);
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: outer.build(),
    }
}

/// Build a request with a single `CreateDb` op (never exempted — always
/// superuser-only).
pub(crate) fn create_db_only_req(db_name: &str, new_db: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("mkdb2");
    b.create_db("mk", shamir_query_builder::ddl::create_db(new_db));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a request with a single `Chmod` op, unsigned (the coarse gate
/// must reject this before the HMAC check is ever reached, since alice
/// can never produce a valid tag for a resource she doesn't own).
pub(crate) fn chmod_only_req(db_name: &str, store: &str, table: &str) -> DbRequest {
    let resource = shamir_query_builder::ddl::res::table(db_name, store, table);
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("cm2");
    b.chmod(
        "cm",
        // Unsigned on purpose: the builder requires a tag, so strip it.
        match shamir_query_builder::ddl::chmod(resource, 0o777)
            .hmac("unused")
            .build()
            .expect("build unsigned chmod")
        {
            shamir_query_types::batch::BatchOp::Chmod(mut op) => {
                op.hmac = None;
                shamir_query_types::batch::BatchOp::Chmod(op)
            }
            other => other,
        },
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a request with a single `CreateUser` op, unsigned.
pub(crate) fn create_user_only_req(db_name: &str, new_user: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("mkuser2");
    b.create_user(
        "cu",
        // Unsigned on purpose: the builder requires a tag, so strip it.
        match shamir_query_builder::ddl::create_user(new_user, "irrelevant-pw")
            .hmac("unused")
            .build()
            .expect("build unsigned create user")
        {
            shamir_query_types::batch::BatchOp::CreateUser(mut op) => {
                op.hmac = None;
                shamir_query_types::batch::BatchOp::CreateUser(op)
            }
            other => other,
        },
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a request with a single `GetBufferConfig` op — one of the 8 ops
/// the REJECTED `is_write()`-based relaxation would have silently
/// exempted; must remain superuser-only under the explicit allowlist.
pub(crate) fn get_buffer_config_only_req(db_name: &str, table: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("gbc");
    b.get_buffer_config("gb", shamir_query_builder::ddl::get_buffer_config(table));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a request with a single `MigrationStatus` op.
pub(crate) fn migration_status_only_req(db_name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("mst");
    b.migration_status("ms", shamir_query_builder::ddl::migration_status("m1"));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a request with a single `InternerDump` op.
pub(crate) fn interner_dump_only_req(db_name: &str) -> DbRequest {
    single_batch_op_req(
        db_name,
        "id",
        shamir_query_builder::ddl::interner_dump().build(),
    )
}

/// Build a request with a single `ChangesSince` op.
pub(crate) fn changes_since_only_req(db_name: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chs");
    b.changes_since("cs", shamir_query_builder::ddl::changes_since(0));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a request with a single `ListValidators` op.
pub(crate) fn list_validators_only_req(db_name: &str, table: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("lv");
    b.list_validators("lv1", shamir_query_builder::ddl::list_validators(table));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a single-entry `BatchRequest` directly from a `BatchOp` produced
/// by a `shamir-query-builder` `ddl::` constructor. Used for the 3
/// replication introspection ops (`ListPublications`/`ListSubscriptions`/
/// `ReplicationStatus`) which have no `Batch`-builder convenience method
/// yet — `BatchRequest`/`QueryEntry` are the query-builder's own public
/// wire DTOs, so this is still "through the builder", not raw JSON.
pub(crate) fn single_batch_op_req(
    db_name: &str,
    alias: &str,
    op: shamir_query_types::batch::BatchOp,
) -> DbRequest {
    let mut queries = shamir_collections::TMap::default();
    queries.insert(
        alias.to_string(),
        shamir_query_types::batch::QueryEntry {
            op,
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let batch = shamir_query_types::batch::BatchRequest {
        id: shamir_types::types::value::QueryValue::Str(alias.to_string()),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries,
        return_all: true,
        return_only: None,
        limits: Default::default(),
        interner_epochs: shamir_collections::TMap::default(),
        result_encoding: Default::default(),
    };
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch,
    }
}

/// Build a request with a single `ListPublications` op.
pub(crate) fn list_publications_only_req(db_name: &str) -> DbRequest {
    single_batch_op_req(
        db_name,
        "lp",
        shamir_query_builder::ddl::list_publications(),
    )
}

/// Build a request with a single `ListSubscriptions` op.
pub(crate) fn list_subscriptions_only_req(db_name: &str) -> DbRequest {
    single_batch_op_req(
        db_name,
        "ls",
        shamir_query_builder::ddl::list_subscriptions(),
    )
}

/// Build a request with a single `ReplicationStatus` op.
pub(crate) fn replication_status_only_req(db_name: &str) -> DbRequest {
    single_batch_op_req(
        db_name,
        "rs",
        shamir_query_builder::ddl::replication_status(),
    )
}

pub(crate) async fn cleanup(
    w: &mut (impl AsyncWriteExt + Unpin),
    r: &mut (impl AsyncReadExt + Unpin),
) {
    let _ = w.shutdown().await;
    let mut tmp = [0u8; 1];
    let _ = r.read(&mut tmp).await;
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------
