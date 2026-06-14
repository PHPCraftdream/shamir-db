//! End-to-end Echo demo (Task 29):
//!
//! - TCP + TLS 1.3 transport
//! - Length-prefix msgpack framing
//! - SCRAM-Argon2id full handshake (`auth_init` → `challenge` → `client_proof` → `auth_ok`)
//! - Server creates a [`Session`] and inserts it into [`SessionStore`]
//! - Client sends N `RequestEnvelope`s containing opaque bytes
//! - Server runs `dispatch_request` (per-spec §7.5 validity check + handler)
//! - Echo handler returns request bytes verbatim
//! - Bumping `tickets_invalid_before_ns` mid-stream causes the next request
//!   to receive `session_invalidated` (spec §7.5 [NORMATIVE]).

use std::sync::Arc;

use rustls::crypto::aws_lc_rs::default_provider;
use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::crypto::{sha256, Ed25519Keypair};
use shamir_connect::common::envelope::{
    ErrorEnvelope, RequestEnvelope, RequestEnvelopeView, ResponseEnvelope,
};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::config::{ListenerPolicy, ServerSecrets};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::{
    dispatch_request_view, DispatchOutcome, HandlerFuture, RequestHandler,
};
use shamir_connect::server::handshake::{
    AuthInitView, ProofOutcome, ServerHandshake, SESSION_MAX_AGE_NS,
};
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};
use shamir_connect::server::user_record::UserRecord;

use shamir_transport_tcp::framing::{
    read_frame, read_frame_into, write_frame, MAX_FRAME_SIZE_DEFAULT,
};
use shamir_transport_tcp::tls::{
    extract_tls_exporter, generate_self_signed_server_cert, make_client_config_no_ca,
    make_server_config_from_pem,
};

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{split, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use zeroize::Zeroizing;

// ---------------------------------------------------------------------------
// Wire frames for the handshake (transport-binding-local, see TRANSPORT_TCP §6).
// Post-handshake messages use `RequestEnvelope`/`ResponseEnvelope` from spec.
// ---------------------------------------------------------------------------

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
}

// ---------------------------------------------------------------------------
// Echo handler — application-level dispatch.
// ---------------------------------------------------------------------------

struct EchoHandler {
    counter: AtomicU64,
}

impl RequestHandler for EchoHandler {
    fn handle<'a>(
        &'a self,
        _session: &'a Session,
        req: &'a [u8],
        _conn: &'a ConnectionServices,
    ) -> HandlerFuture<'a> {
        self.counter.fetch_add(1, Ordering::Relaxed);
        let out = req.to_vec();
        Box::pin(async move { Ok(out) })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn make_user(password: &[u8]) -> (UserRecord, NormalizedUsername) {
    let username = NormalizedUsername::from_raw("alice").unwrap();
    let salt = [0x42u8; 16];
    let kdf = fast_kdf();
    let derived = DerivedKeys::derive(password, &salt, &kdf).unwrap();
    let mut server_key_z: Zeroizing<[u8; 32]> = Zeroizing::new([0u8; 32]);
    server_key_z.copy_from_slice(&derived.server_key[..]);
    let record = UserRecord {
        salt,
        stored_key: derived.stored_key,
        server_key: server_key_z,
        kdf_params: kdf,
        tickets_invalid_before_ns: 0,
    };
    (record, username)
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn echo_full_pipeline_with_session_and_invalidation() {
    let _ = default_provider().install_default();

    // ---- Fixtures ----
    let identity = Arc::new(Ed25519Keypair::generate());
    let secrets = Arc::new(ServerSecrets {
        server_secret: [0xaa; 32],
        lockout_secret: [0xbb; 32],
    });
    let listener_policy = ListenerPolicy::new(BindingMode::TlsExporter);
    let password: &[u8] = b"swordfish";
    let (user_record, username) = make_user(password);
    let server_pub = identity.public_bytes();
    let pinned_hash = sha256(&server_pub);

    let session_store = Arc::new(SessionStore::new());
    let echo = Arc::new(EchoHandler {
        counter: AtomicU64::new(0),
    });

    // For §7.5 invalidation demo:
    let tickets_invalid_before_ns = Arc::new(AtomicU64::new(0));

    // ---- TLS ----
    let (cert_pem, key_pem) = generate_self_signed_server_cert(vec!["localhost".into()]).unwrap();
    let server_cfg = make_server_config_from_pem(&cert_pem, &key_pem).unwrap();
    let client_cfg = make_client_config_no_ca();

    // ---- Listener ----
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(server_cfg);

    // ---- Server task ----
    let server_identity = identity.clone();
    let server_secrets = secrets.clone();
    let server_user = user_record.clone();
    let server_username = username.clone();
    let server_session_store = session_store.clone();
    let server_echo = echo.clone();
    let server_tib = tickets_invalid_before_ns.clone();
    let alice_uid = [0x01u8; 16];

    let server_task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let tls = acceptor.accept(tcp).await.unwrap();
        let exporter = extract_tls_exporter(&tls).unwrap();
        let (mut r, mut w) = split(tls);

        // ----- 1. Handshake -----
        // Spec §8.5 NORMATIVE latency padding: capture wall-clock at the
        // start of the auth flow; before emitting `auth_ok` (or any
        // negative response) we sleep until target_constant_time_ms is
        // reached. Defeats the real-vs-fake user timing oracle that
        // branch-equivalent code alone cannot close (SECURITY_MODEL §9.2).
        use shamir_connect::common::latency::{target_constant_time_ms, LatencyPadGuard};
        let pad_guard = LatencyPadGuard::start();

        let init_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
        let init: WireAuthInit = rmp_serde::from_slice(&init_bytes).unwrap();
        let mut client_nonce = [0u8; 32];
        client_nonce.copy_from_slice(&init.client_nonce);
        let init_view = AuthInitView {
            user: NormalizedUsername::from_raw(&init.user).unwrap(),
            client_nonce,
            binding_mode: BindingMode::from_u8(init.binding_mode).unwrap(),
            version: init.version,
        };

        let lookup = |u: &NormalizedUsername| -> Option<UserRecord> {
            if u.as_str() == server_username.as_str() {
                Some(server_user.clone())
            } else {
                None
            }
        };
        let hs = ServerHandshake::new(
            listener_policy,
            TransportKind::Tcp,
            &server_secrets,
            init_view,
            exporter,
            fast_kdf(),
            lookup,
        )
        .unwrap();

        let ch = hs.challenge();
        let bytes = rmp_serde::to_vec(&WireChallenge {
            salt: ch.salt.to_vec(),
            memory_kb: ch.kdf_params.memory_kb,
            time: ch.kdf_params.time,
            parallelism: ch.kdf_params.parallelism,
            argon2_version: ch.kdf_params.argon2_version,
            server_nonce: ch.server_nonce.to_vec(),
        })
        .unwrap();
        write_frame(&mut w, &bytes).await.unwrap();

        let proof_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
        let proof_msg: WireClientProof = rmp_serde::from_slice(&proof_bytes).unwrap();
        let mut proof_arr = [0u8; 32];
        proof_arr.copy_from_slice(&proof_msg.client_proof);

        let ok = match hs
            .verify_proof(&proof_arr, &server_identity, SESSION_MAX_AGE_NS)
            .unwrap()
        {
            ProofOutcome::Accepted(ok) => *ok,
            ProofOutcome::Rejected => panic!("server rejected proof"),
        };

        // Insert into SessionStore so dispatch_request can find it.
        let now_ns = UnixNanos::now().as_u64();
        let session = Session::new(
            alice_uid,
            server_username.as_str().to_string(),
            SessionPermissions::from_roles(vec!["read_write".into()]),
            TransportKind::Tcp,
            BindingMode::TlsExporter,
            exporter,
            now_ns,
        );
        server_session_store.insert(ok.session_id, session);

        let bytes = rmp_serde::to_vec(&WireAuthOk {
            server_signature: ok.server_signature.to_vec(),
            server_pub_key: ok.server_pub_key.to_vec(),
            identity_sig: ok.identity_sig.to_vec(),
            session_id: ok.session_id.to_vec(),
            expires_at_ns: ok.expires_at_ns,
        })
        .unwrap();
        // Spec §8.5: pad to target_constant_time_ms BEFORE writing auth_ok.
        let pad = pad_guard.finish_with_target(target_constant_time_ms());
        if pad > std::time::Duration::ZERO {
            tokio::time::sleep(pad).await;
        }
        write_frame(&mut w, &bytes).await.unwrap();

        // ----- 2. Echo loop -----
        // Per-connection scratch buffer + zero-copy envelope view = the
        // production hot-path pattern:
        //   - `read_frame_into` reuses buffer capacity (Optim #1).
        //   - `RequestEnvelopeView::from_msgpack` borrows session_id + req
        //     directly from `frame` (Optim #4).
        //   - `dispatch_request_view` skips the owning Vec<u8> allocations.
        // Combined: ~½ the allocator pressure per request vs the owning APIs.
        let mut frame: Vec<u8> = Vec::with_capacity(4096);
        loop {
            match read_frame_into(&mut r, MAX_FRAME_SIZE_DEFAULT, &mut frame).await {
                Ok(()) => {}
                Err(_) => break, // client closed
            }
            let view = RequestEnvelopeView::from_msgpack(&frame).unwrap();
            let conn = ConnectionServices::without_push(0);
            let outcome = dispatch_request_view(
                &view,
                &server_session_store,
                |_uid| server_tib.load(Ordering::Relaxed),
                &*server_echo,
                &conn,
            )
            .await
            .unwrap();
            let reply_bytes = match outcome {
                DispatchOutcome::Response(r) => r.to_msgpack().unwrap(),
                DispatchOutcome::Error(e) => e.to_msgpack().unwrap(),
            };
            write_frame(&mut w, &reply_bytes).await.unwrap();
        }
        let _ = w.shutdown().await;
        let mut tmp = [0u8; 1];
        let _ = r.read(&mut tmp).await;
    });

    // ---- Client side ----
    let connector = TlsConnector::from(client_cfg);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let tls = connector.connect(server_name, tcp).await.unwrap();
    let exporter = extract_tls_exporter(&tls).unwrap();
    let (mut r, mut w) = split(tls);

    let hs = HandshakeBuilder::new(
        username.clone(),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
    )
    .tls_exporter(exporter)
    .pinned_hash(pinned_hash)
    .build()
    .unwrap();

    // auth_init
    let init = hs.auth_init();
    let bytes = rmp_serde::to_vec(&WireAuthInit {
        user: init.user,
        client_nonce: init.client_nonce.to_vec(),
        binding_mode: init.binding_mode,
        version: init.version,
    })
    .unwrap();
    write_frame(&mut w, &bytes).await.unwrap();

    // challenge
    let ch_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
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
    let mut password_buf = password.to_vec();
    let (proof, derived, am) = hs.process_challenge(&challenge, &mut password_buf).unwrap();

    // proof
    let bytes = rmp_serde::to_vec(&WireClientProof {
        client_proof: proof.to_vec(),
    })
    .unwrap();
    write_frame(&mut w, &bytes).await.unwrap();

    // auth_ok
    let ok_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
    let ok_wire: WireAuthOk = rmp_serde::from_slice(&ok_bytes).unwrap();
    let mut sig32 = [0u8; 32];
    sig32.copy_from_slice(&ok_wire.server_signature);
    let mut pub32 = [0u8; 32];
    pub32.copy_from_slice(&ok_wire.server_pub_key);
    let mut id_sig = [0u8; 64];
    id_sig.copy_from_slice(&ok_wire.identity_sig);
    let mut sid = [0u8; 32];
    sid.copy_from_slice(&ok_wire.session_id);
    let auth_ok = ServerAuthOk {
        server_signature: sig32,
        server_pub_key: pub32,
        identity_sig: id_sig,
        session_id: sid,
        expires_at_ns: ok_wire.expires_at_ns,
        resumption_ticket: None,
        resumption_expires_at_ns: None,
        rotation_in_progress: None,
        kdf_upgrade_required: None,
    };
    let success = hs.process_auth_ok(&auth_ok, &derived, &am, |_| {}).unwrap();
    assert_eq!(success.session_id, sid);

    // ----- 3. Echo round trips -----
    for i in 0..5u32 {
        let payload = format!("ping {i}").into_bytes();
        let env = RequestEnvelope::new(sid, Some(i), payload.clone());
        let bytes = env.to_msgpack().unwrap();
        write_frame(&mut w, &bytes).await.unwrap();

        let reply_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
        let resp = ResponseEnvelope::from_msgpack(&reply_bytes).unwrap();
        assert_eq!(resp.request_id, Some(i));
        assert_eq!(resp.res, payload);
    }
    assert_eq!(echo.counter.load(Ordering::Relaxed), 5);

    // ----- 4. Bump tickets_invalid_before_ns → next request must receive
    //          session_invalidated (per spec §7.5).
    let now_ns = UnixNanos::now().as_u64();
    tickets_invalid_before_ns.store(now_ns + 60, Ordering::Relaxed);

    let env = RequestEnvelope::new(sid, Some(99), b"after-bump".to_vec());
    write_frame(&mut w, &env.to_msgpack().unwrap())
        .await
        .unwrap();
    let reply_bytes = read_frame(&mut r, MAX_FRAME_SIZE_DEFAULT).await.unwrap();
    let err = ErrorEnvelope::from_msgpack(&reply_bytes).unwrap();
    assert_eq!(err.request_id, Some(99));
    assert_eq!(err.error, "session_invalidated");

    // Session must be removed from store on §7.5 trigger.
    assert_eq!(session_store.len(), 0);

    let _ = w.shutdown().await;
    let _ = server_task.await;
}
