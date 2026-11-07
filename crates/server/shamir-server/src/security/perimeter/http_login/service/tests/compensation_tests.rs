use super::fixture::Fixture;
use crate::http_login::admission::Source;
use crate::http_login::pending::{PendingEntry, PendingId};
use crate::http_login::service::{begin, finish};
use crate::http_login::wire::response as wire_response;
use axum::http::StatusCode;
use shamir_connect::common::crypto::{hmac_sha256, sha256, StoredKey};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::audit_chain::{AuditAppender, AuditEntry};
use shamir_connect::server::http_login::{build_transcript, HttpLoginTranscriptInput};
use shamir_connect::server::token::TokenId;
use shamir_connect::server::user_record::UserRecord;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

const USER: &str = "compensation-user";
const CLIENT_KEY: [u8; 32] = [7; 32];
const SERVER_KEY: [u8; 32] = [8; 32];
const SCOPE: [u8; 6] = [0, 1, b'x', 0, 0, 0xff];
const REDACTION_SENTINEL: &str = "blocking redaction sentinel";

#[derive(Clone, Default)]
struct CompensationTraceCapture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for CompensationTraceCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("capture lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CompensationTraceCapture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

struct BlockingRedactionAppender {
    test_thread: std::thread::ThreadId,
}
impl AuditAppender for BlockingRedactionAppender {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        assert_ne!(std::thread::current().id(), self.test_thread);
        if entry.event == "api_token_created" {
            tracing::info!(event = "api_token_created", "blocking redaction sentinel");
        }
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

struct FloorBumpAppender {
    users: Arc<crate::security::user_directory::FjallUserDirectory>,
    fired: Arc<AtomicBool>,
}
impl AuditAppender for FloorBumpAppender {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        if entry.event == "api_token_created" {
            assert!(self
                .users
                .bump_tickets_invalid(USER, 90_000_000_001)
                .expect("raise ticket floor after durable create"));
            self.fired.store(true, Ordering::Release);
        }
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}
struct RoleChangeAppender {
    users: Arc<crate::security::user_directory::FjallUserDirectory>,
    fired: Arc<AtomicBool>,
}
impl AuditAppender for RoleChangeAppender {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        if entry.event == "api_token_created" {
            assert!(UserDirectory::update_roles(
                self.users.as_ref(),
                USER,
                vec!["reader".into()],
                0,
            )
            .expect("change roles after durable create"));
            self.fired.store(true, Ordering::Release);
        }
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}
fn source() -> Source {
    Source::from_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
}
fn finish_body(id: &[u8], proof: &[u8]) -> Vec<u8> {
    let mut body = vec![0x83];
    for (key, value) in [
        ("protocol_version", vec![1]),
        ("handshake_id", binary(id)),
        ("client_proof", binary(proof)),
    ] {
        body.push(0xa0 | u8::try_from(key.len()).expect("short key"));
        body.extend_from_slice(key.as_bytes());
        body.extend_from_slice(&value);
    }
    body
}
fn binary(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0xc4, u8::try_from(bytes.len()).expect("short binary")];
    out.extend_from_slice(bytes);
    out
}
fn encoded_string(value: &str) -> Vec<u8> {
    let mut v = vec![0xa0 | u8::try_from(value.len()).expect("short string")];
    v.extend_from_slice(value.as_bytes());
    v
}
fn pair(body: &mut Vec<u8>, key: &str, value: &[u8]) {
    body.extend_from_slice(&encoded_string(key));
    body.extend_from_slice(value);
}
fn begin_body() -> Vec<u8> {
    let mut scope = vec![0x84];
    pair(&mut scope, "label", &encoded_string("x"));
    pair(&mut scope, "mode", &encoded_string("read_only"));
    pair(&mut scope, "channels", &encoded_string("rest"));
    pair(&mut scope, "databases", &[0xc0]);
    let mut b = vec![0x85];
    pair(&mut b, "protocol_version", &[1]);
    pair(&mut b, "user", &encoded_string(USER));
    pair(&mut b, "client_nonce", &binary(&[1; 32]));
    pair(&mut b, "binding_mode", &[3]);
    pair(&mut b, "token", &scope);
    b
}
#[derive(serde::Deserialize)]
struct Challenge {
    handshake_id: serde_bytes::ByteBuf,
    server_nonce: serde_bytes::ByteBuf,
}
#[derive(serde::Deserialize)]
struct Issued {
    token: String,
    #[serde(default)]
    id: Option<serde_bytes::ByteBuf>,
}

#[tokio::test(start_paused = true)]
async fn begin_finish_redaction_captures_blocking_audit_control() {
    let test_thread = std::thread::current().id();
    let capture = CompensationTraceCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(capture.clone())
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let _default_guard = tracing::dispatcher::set_default(&dispatch);
    let fixture =
        Fixture::with_token_audit(move |_| Arc::new(BlockingRedactionAppender { test_thread }))
            .await;
    let params = KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    };
    UserDirectory::insert(
        fixture.context.users.as_ref(),
        USER.to_owned(),
        UserRecord {
            salt: [4; 16],
            stored_key: StoredKey(sha256(&CLIENT_KEY)),
            server_key: Zeroizing::new(SERVER_KEY),
            kdf_params: params,
            tickets_invalid_before_ns: 0,
        },
    )
    .expect("insert user");
    let client_nonce = [1; 32];
    let stored_key = sha256(&CLIENT_KEY);
    let captured = async {
        tracing::info!("BEGIN redaction oracle");
        let response = begin(&fixture.context, source(), &begin_body()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let challenge: Challenge = rmp_serde::from_slice(response.body()).expect("challenge");
        let id = challenge.handshake_id.into_vec();
        let server_nonce = challenge.server_nonce.into_vec();
        assert_eq!(id.len(), 32);
        assert_eq!(server_nonce.len(), 32);
        let id: [u8; 32] = id.try_into().expect("fixed handshake id");
        let server_nonce: [u8; 32] = server_nonce.try_into().expect("fixed server nonce");
        let transcript = build_transcript(&HttpLoginTranscriptInput {
            protocol_version: 1,
            binding_mode: 3,
            security: fixture.context.security_class,
            listener_id: &fixture.context.listener_id,
            handshake_id: &id,
            user: USER,
            client_nonce: &client_nonce,
            server_nonce: &server_nonce,
            salt: &[4; 16],
            memory_kb: params.memory_kb,
            time: params.time,
            parallelism: params.parallelism,
            argon2_version: params.argon2_version,
            scope_bytes: &SCOPE,
        })
        .expect("transcript");
        let signature = hmac_sha256(&stored_key, &transcript);
        let proof: [u8; 32] = core::array::from_fn(|i| CLIENT_KEY[i] ^ signature[i]);
        let response = finish(&fixture.context, source(), &finish_body(&id, &proof)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let issued: Issued = rmp_serde::from_slice(response.body()).expect("issued token");
        assert!(!issued.token.is_empty());
        assert_eq!(issued.id.as_ref().map(|id| id.len()), Some(16));
        let token_secret = issued
            .token
            .split('.')
            .next_back()
            .unwrap_or(&issued.token)
            .to_owned();
        let malformed = begin(&fixture.context, source(), &[]).await;
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
        let replay = finish(&fixture.context, source(), &finish_body(&id, &proof)).await;
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
        (
            id,
            proof.to_vec(),
            client_nonce.to_vec(),
            server_nonce,
            issued.token,
            token_secret,
        )
    }
    .await;
    let logs =
        String::from_utf8(capture.0.lock().expect("capture lock").clone()).expect("UTF-8 logs");
    assert!(logs.contains(REDACTION_SENTINEL));
    assert!(logs.contains("BEGIN redaction oracle"));
    assert!(logs.contains("api_token_created"));
    drop(_default_guard);
    let (id, proof, client_nonce, server_nonce, token, token_secret) = captured;
    let secrets = [
        USER.to_owned(),
        source().address().to_string(),
        hex::encode(id),
        hex::encode(proof),
        token,
        token_secret,
        hex::encode(client_nonce),
        hex::encode(server_nonce),
        hex::encode(sha256(&CLIENT_KEY)),
        hex::encode(SERVER_KEY),
    ];
    for secret in secrets {
        assert!(!logs.contains(&secret), "secret leaked to captured logs");
    }
}

async fn assert_floor_bump_compensates(compensation_now: Option<u64>, expected_revoke: u64) {
    let fired = Arc::new(AtomicBool::new(false));
    let appender_fired = Arc::clone(&fired);
    let mut fixture = Fixture::with_token_audit(move |users| {
        Arc::new(FloorBumpAppender {
            users,
            fired: appender_fired,
        })
    })
    .await;
    let clock_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&clock_calls);
    fixture.context.unix_clock = Arc::new(move || {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Some(90_000_000_000)
        } else {
            compensation_now
        }
    });
    let id = [0x91; 32];
    let params = KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    };
    let users = Arc::clone(&fixture.context.users);
    let user_id = UserDirectory::insert(
        users.as_ref(),
        USER.to_owned(),
        UserRecord {
            salt: [4; 16],
            stored_key: StoredKey(sha256(&CLIENT_KEY)),
            server_key: Zeroizing::new(SERVER_KEY),
            kdf_params: params,
            tickets_invalid_before_ns: 0,
        },
    )
    .expect("insert test user");
    let transcript = build_transcript(&HttpLoginTranscriptInput {
        protocol_version: 1,
        binding_mode: 3,
        security: fixture.context.security_class,
        listener_id: &fixture.context.listener_id,
        handshake_id: &id,
        user: USER,
        client_nonce: &[1; 32],
        server_nonce: &[2; 32],
        salt: &[4; 16],
        memory_kb: params.memory_kb,
        time: params.time,
        parallelism: params.parallelism,
        argon2_version: params.argon2_version,
        scope_bytes: &SCOPE,
    })
    .expect("build transcript");
    let signature = hmac_sha256(&sha256(&CLIENT_KEY), &transcript);
    let proof: [u8; 32] = core::array::from_fn(|i| CLIENT_KEY[i] ^ signature[i]);
    fixture
        .context
        .pending
        .insert(
            PendingId(id),
            PendingEntry {
                transcript: Zeroizing::new(transcript.clone()),
                user_id: Some(user_id),
                stored_key: Zeroizing::new(sha256(&CLIENT_KEY)),
                server_key: Zeroizing::new(SERVER_KEY),
                tickets_invalid_before_ns: 0,
                begin_time_ns: 1,
                eligible: true,
                listener_id: fixture.context.listener_id,
                security_class: fixture.context.security_class,
                source: source(),
            },
        )
        .await
        .expect("insert pending entry");
    let response = finish(&fixture.context, source(), &finish_body(&id, &proof)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.body(),
        wire_response(401, "authentication_failed", None)
            .expect("canonical unauthorized response")
            .body()
    );
    assert_eq!(clock_calls.load(Ordering::SeqCst), 2);
    assert!(fired.load(Ordering::Acquire));
    assert_eq!(fixture.lockout.failure_pair_count(), 0);
    assert_eq!(fixture.context.pending.count().await, 0);
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 1);
    let records = fixture
        .context
        .tokens
        .store_for_tests()
        .scan()
        .await
        .expect("scan persisted tokens");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].created_at_ns, 90_000_000_000);
    assert_eq!(records[0].revoked_at_ns, Some(expected_revoke));
    let token_id = TokenId(records[0].id);
    let mirrored = fixture
        .context
        .tokens
        .mirror()
        .get(&token_id)
        .expect("persisted token must be mirrored");
    assert!(mirrored.is_revoked());
    assert_eq!(mirrored.revoked_at_ns(), Some(expected_revoke));
    assert_eq!(
        fixture.context.tokens.mirror().ids_of_owner(&user_id),
        vec![token_id]
    );
    assert_eq!(
        fixture
            .context
            .tokens
            .active_count(90_000_000_000)
            .await
            .expect("count active tokens"),
        0
    );
}
#[tokio::test(start_paused = true)]
async fn role_change_after_create_compensates_token() {
    let fired = Arc::new(AtomicBool::new(false));
    let appender_fired = Arc::clone(&fired);
    let mut fixture = Fixture::with_token_audit(move |users| {
        Arc::new(RoleChangeAppender {
            users,
            fired: appender_fired,
        })
    })
    .await;
    let clock_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&clock_calls);
    fixture.context.unix_clock = Arc::new(move || {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Some(90_000_000_000)
        } else {
            Some(91_000_000_000)
        }
    });
    let params = KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    };
    let users = Arc::clone(&fixture.context.users);
    let user_id = UserDirectory::insert(
        users.as_ref(),
        USER.to_owned(),
        UserRecord {
            salt: [4; 16],
            stored_key: StoredKey(sha256(&CLIENT_KEY)),
            server_key: Zeroizing::new(SERVER_KEY),
            kdf_params: params,
            tickets_invalid_before_ns: 0,
        },
    )
    .expect("insert test user");
    let id = [0x91; 32];
    let transcript = build_transcript(&HttpLoginTranscriptInput {
        protocol_version: 1,
        binding_mode: 3,
        security: fixture.context.security_class,
        listener_id: &fixture.context.listener_id,
        handshake_id: &id,
        user: USER,
        client_nonce: &[1; 32],
        server_nonce: &[2; 32],
        salt: &[4; 16],
        memory_kb: params.memory_kb,
        time: params.time,
        parallelism: params.parallelism,
        argon2_version: params.argon2_version,
        scope_bytes: &SCOPE,
    })
    .expect("build transcript");
    let signature = hmac_sha256(&sha256(&CLIENT_KEY), &transcript);
    let proof: [u8; 32] = core::array::from_fn(|i| CLIENT_KEY[i] ^ signature[i]);
    fixture
        .context
        .pending
        .insert(
            PendingId(id),
            PendingEntry {
                transcript: Zeroizing::new(transcript),
                user_id: Some(user_id),
                stored_key: Zeroizing::new(sha256(&CLIENT_KEY)),
                server_key: Zeroizing::new(SERVER_KEY),
                tickets_invalid_before_ns: 0,
                begin_time_ns: 1,
                eligible: true,
                listener_id: fixture.context.listener_id,
                security_class: fixture.context.security_class,
                source: source(),
            },
        )
        .await
        .expect("insert pending entry");
    let response = finish(&fixture.context, source(), &finish_body(&id, &proof)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.body(),
        wire_response(401, "authentication_failed", None)
            .expect("canonical unauthorized response")
            .body()
    );
    assert_eq!(clock_calls.load(Ordering::SeqCst), 2);
    assert!(fired.load(Ordering::Acquire));
    assert_eq!(fixture.lockout.failure_pair_count(), 0);
    assert_eq!(fixture.context.pending.count().await, 0);
    let records = fixture
        .context
        .tokens
        .store_for_tests()
        .scan()
        .await
        .expect("scan persisted tokens");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].created_at_ns, 90_000_000_000);
    assert_eq!(records[0].revoked_at_ns, Some(91_000_000_000));
    assert_eq!(
        fixture
            .context
            .tokens
            .active_count(90_000_000_000)
            .await
            .expect("count active tokens"),
        0
    );
}

#[tokio::test(start_paused = true)]
async fn floor_bump_after_create_compensates_token_at_refreshed_time() {
    assert_floor_bump_compensates(Some(91_000_000_000), 91_000_000_000).await;
}
#[tokio::test(start_paused = true)]
async fn floor_bump_after_create_uses_created_time_when_clock_unavailable() {
    assert_floor_bump_compensates(None, 90_000_000_000).await;
}
#[tokio::test(start_paused = true)]
async fn floor_bump_after_create_uses_created_time_when_clock_moves_backward() {
    assert_floor_bump_compensates(Some(89_000_000_000), 90_000_000_000).await;
}
