//! End-to-end permission enforcement tests (Shomer access fabric).
//!
//! Exercises the full wire path: SCRAM-authenticated session →
//! `ShamirDbHandler::execute` → `ShamirDb::execute_as` → `authorize_access`.
//!
//! Scenarios:
//!   1. **deny/allow by mode** — admin creates table, chmod 0o700 (owner-only);
//!      non-admin user is DENIED; admin is ALLOWED.
//!   2. **group grant** — admin creates group, adds user, chgrp + chmod group-read;
//!      user is now ALLOWED via group; non-member is DENIED.
//!   3. **open default** — new resources default to 0o777; any authenticated
//!      user is ALLOWED.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::io::{split, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::{extract_tls_exporter, make_client_config_no_ca};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, SecurityConfig,
    TlsConfig,
};
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::server::{BootstrapMode, ServerLauncher};
use shamir_server::version::CURRENT_QUERY_LANG_VERSION;

// --------------------------------------------------------------------------
// Wire-frame mirrors (same as mvp_e2e.rs)
// --------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct WireAuthInit {
    user: String,
    #[serde(with = "serde_bytes")]
    client_nonce: Vec<u8>,
    binding_mode: u8,
    version: u8,
}

#[derive(Serialize, Deserialize)]
struct WireChallenge {
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
struct WireClientProof {
    #[serde(with = "serde_bytes")]
    client_proof: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct WireAuthOk {
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

fn fast_kdf() -> KdfConfig {
    KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn make_test_config(temp: &TempDir) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
    Config {
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
        },
        replication: None,
    }
}

async fn roundtrip<R, W>(
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
async fn scram_login(
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

    let client_cfg = make_client_config_no_ca();
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
fn read_batch(table: &str) -> shamir_db::query::batch::BatchRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("read");
    b.query("rd", shamir_query_builder::Query::from(table));
    b.build()
}

/// Build a batch with a single chmod op on a table. `session_id` is the
/// signing session's bearer token — chmod is HMAC-gated (task #542).
fn chmod_batch(session_id: [u8; 32], db: &str, store: &str, table: &str, mode: u16) -> DbRequest {
    let resource = shamir_query_builder::ddl::res::table(db, store, table);
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_chmod(&resource, mode),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chmod");
    b.chmod(
        "cm",
        shamir_query_builder::ddl::chmod(resource, mode).hmac(tag),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch with a single chmod op on a database (top-level container).
/// `session_id` is the signing session's bearer token.
fn chmod_db_batch(session_id: [u8; 32], db: &str, mode: u16) -> DbRequest {
    let resource = shamir_query_builder::ddl::res::database(db);
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_chmod(&resource, mode),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chmod_db");
    b.chmod(
        "cm",
        shamir_query_builder::ddl::chmod(resource, mode).hmac(tag),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch with a single chmod op on a store/repo. `session_id` is
/// the signing session's bearer token.
fn chmod_store_batch(session_id: [u8; 32], db: &str, store: &str, mode: u16) -> DbRequest {
    let resource = shamir_query_builder::ddl::res::store(db, store);
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_chmod(&resource, mode),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chmod_store");
    b.chmod(
        "cm",
        shamir_query_builder::ddl::chmod(resource, mode).hmac(tag),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch with a single chgrp op on a table. `session_id` is the
/// signing session's bearer token — chgrp is HMAC-gated (task #542).
fn chgrp_batch(session_id: [u8; 32], db: &str, store: &str, table: &str, group: u64) -> DbRequest {
    let resource = shamir_query_builder::ddl::res::table(db, store, table);
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_chgrp(&resource, Some(group)),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("chgrp");
    b.chgrp(
        "cg",
        shamir_query_builder::ddl::chgrp(resource, Some(group)).hmac(tag),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch creating a group. `session_id` is the signing session's
/// bearer token — create_group is HMAC-gated (task #551).
fn create_group_req(session_id: [u8; 32], db: &str, name: &str) -> DbRequest {
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_create_group(name),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("mkgrp");
    b.create_group(
        "mk",
        shamir_query_builder::ddl::create_group(name).hmac(tag),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a batch adding a user to a group. `session_id` is the signing
/// session's bearer token — add_group_member is HMAC-gated (task #551).
fn add_group_member_req(
    session_id: [u8; 32],
    db: &str,
    group_name: &str,
    user_id: u64,
) -> DbRequest {
    let group = shamir_query_builder::ddl::GroupRef::Name {
        name: group_name.to_string(),
    };
    let key = shamir_query_types::hmac::derive_session_hmac_key(&session_id);
    let tag = shamir_query_types::hmac::compute_tag_hex(
        &key,
        &shamir_query_types::hmac::canonical_add_group_member(&group, user_id),
    );
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("addmember");
    b.add_group_member(
        "am",
        shamir_query_builder::ddl::add_group_member(group, user_id).hmac(tag),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        batch: b.build(),
    }
}

/// Build a create_db request.
fn create_db_req(db_name: &str) -> DbRequest {
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
fn create_repo_table_req(db_name: &str, table_name: &str) -> DbRequest {
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
fn create_table_only_req(db_name: &str, table_name: &str) -> DbRequest {
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
fn read_req(db_name: &str, table_name: &str) -> DbRequest {
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: read_batch(table_name),
    }
}

/// Build a single-`DescribeTable`-op `DbRequest::Execute` (task #553
/// coarse-gate allowlist tests).
fn describe_table_req(db_name: &str, table: &str) -> DbRequest {
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
fn get_table_schema_req(db_name: &str, table: &str) -> DbRequest {
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
fn access_tree_req(db_name: &str) -> DbRequest {
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
fn list_databases_req(db_name: &str) -> DbRequest {
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
fn list_users_req(db_name: &str) -> DbRequest {
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
fn nested_batch_read_req(db_name: &str, table: &str) -> DbRequest {
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
fn create_db_only_req(db_name: &str, new_db: &str) -> DbRequest {
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
fn chmod_only_req(db_name: &str, store: &str, table: &str) -> DbRequest {
    let resource = shamir_query_builder::ddl::res::table(db_name, store, table);
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("cm2");
    b.chmod("cm", shamir_query_builder::ddl::chmod(resource, 0o777));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db_name.into(),
        batch: b.build(),
    }
}

/// Build a request with a single `CreateUser` op, unsigned.
fn create_user_only_req(db_name: &str, new_user: &str) -> DbRequest {
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("mkuser2");
    b.create_user(
        "cu",
        shamir_query_builder::ddl::create_user(new_user, "irrelevant-pw"),
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
fn get_buffer_config_only_req(db_name: &str, table: &str) -> DbRequest {
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
fn migration_status_only_req(db_name: &str) -> DbRequest {
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
fn interner_dump_only_req(db_name: &str) -> DbRequest {
    single_batch_op_req(
        db_name,
        "id",
        shamir_query_builder::ddl::interner_dump().build(),
    )
}

/// Build a request with a single `ChangesSince` op.
fn changes_since_only_req(db_name: &str) -> DbRequest {
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
fn list_validators_only_req(db_name: &str, table: &str) -> DbRequest {
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
fn single_batch_op_req(
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
        },
    );
    let batch = shamir_query_types::batch::BatchRequest {
        id: shamir_types::types::value::QueryValue::Str(alias.to_string()),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
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
fn list_publications_only_req(db_name: &str) -> DbRequest {
    single_batch_op_req(
        db_name,
        "lp",
        shamir_query_builder::ddl::list_publications(),
    )
}

/// Build a request with a single `ListSubscriptions` op.
fn list_subscriptions_only_req(db_name: &str) -> DbRequest {
    single_batch_op_req(
        db_name,
        "ls",
        shamir_query_builder::ddl::list_subscriptions(),
    )
}

/// Build a request with a single `ReplicationStatus` op.
fn replication_status_only_req(db_name: &str) -> DbRequest {
    single_batch_op_req(
        db_name,
        "rs",
        shamir_query_builder::ddl::replication_status(),
    )
}

async fn cleanup(w: &mut (impl AsyncWriteExt + Unpin), r: &mut (impl AsyncReadExt + Unpin)) {
    let _ = w.shutdown().await;
    let mut tmp = [0u8; 1];
    let _ = r.read(&mut tmp).await;
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

/// Scenario 1: deny non-owner, allow owner when mode is 0o700.
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
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "alice".into(),
            password: String::from_utf8(user_pw.clone()).expect("utf8"),
            roles: vec![],
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
        let resp = roundtrip(
            &DbRequest::CreateScramUser {
                name: name.into(),
                password: String::from_utf8(pw.clone()).expect("utf8"),
                roles: vec![],
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
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "dave".into(),
            password: String::from_utf8(dave_pw.clone()).expect("utf8"),
            roles: vec![],
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
// `docs/prompts/audit/71-wire-admin-gate-explicit-allowlist.md`:
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
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "alice".into(),
            password: String::from_utf8(alice_pw.clone()).expect("utf8"),
            roles: vec![],
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
/// the now-passing coarse gate); `List(Databases)` is allowed (Root's
/// default-open List/Read traversal per the #552 posture).
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
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "bob553c".into(),
            password: String::from_utf8(bob_pw.clone()).expect("utf8"),
            roles: vec![],
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

    // (d) List(Databases) — Root's default mode permits List/Read
    // traversal, so a non-superuser is ALLOWED.
    let resp = roundtrip(
        &list_databases_req(db_name),
        bob_sid,
        &mut bob_rid,
        &mut bob_w,
        &mut bob_r,
    )
    .await;
    assert!(
        matches!(resp, DbResponse::Batch { .. }),
        "List(Databases) should be allowed for a non-superuser: {:?}",
        resp
    );

    cleanup(&mut bob_w, &mut bob_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}

/// (e) THE regression test: a nested `Batch{ Read(forbidden_table) }` must
/// stay DENIED for a non-superuser. `Batch` is deliberately excluded from
/// the allowlist — if it were exempted (as the rejected `is_write()`-based
/// relaxation would have done), the nested `Read` would execute with ZERO
/// per-table authorization, since `required_access(Batch) == None` and the
/// per-op authorization loop never recurses into `SubBatchOp`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn coarse_gate_nested_batch_stays_denied() {
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
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "carol553e".into(),
            password: String::from_utf8(carol_pw.clone()).expect("utf8"),
            roles: vec![],
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

    // Carol sends Batch{ sub: Read(secret_table) }. `Batch` is NOT in the
    // allowlist, so `is_admin(Batch) == true && !exempt` → the whole
    // request is rejected by the coarse gate itself, exactly as today.
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
                code, "permission_denied",
                "nested Batch{{Read(forbidden)}} must be rejected by the coarse gate, got code={}, msg={}",
                code, message
            );
        }
        other => panic!(
            "expected permission_denied for nested Batch{{Read(forbidden)}}, got {:?}",
            other
        ),
    }

    cleanup(&mut carol_w, &mut carol_r).await;
    cleanup(&mut admin_w, &mut admin_r).await;
    handle.shutdown().await;
}

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
    let resp = roundtrip(
        &DbRequest::CreateScramUser {
            name: "eve553f".into(),
            password: String::from_utf8(eve_pw.clone()).expect("utf8"),
            roles: vec![],
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
