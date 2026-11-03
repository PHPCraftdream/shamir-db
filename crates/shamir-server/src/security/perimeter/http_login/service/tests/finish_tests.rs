use super::fixture::Fixture;
use crate::http_login::admission::Source;
use crate::http_login::pending::{PendingEntry, PendingId};
use crate::http_login::service::finish;
use crate::http_login::wire::response as wire_response;
use axum::http::StatusCode;
use shamir_connect::common::crypto::{hmac_sha256, sha256, StoredKey};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::http_login::{
    build_server_signature, build_transcript, HttpLoginTranscriptInput,
};
use shamir_connect::server::lockout::LockoutStore;
use shamir_connect::server::token::parse_token;
use shamir_connect::server::user_record::UserRecord;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use tracing::instrument::WithSubscriber;
use zeroize::Zeroizing;

const USER: &str = "finish-user";
const CLIENT_KEY: [u8; 32] = [7; 32];
const SERVER_KEY: [u8; 32] = [8; 32];
const SCOPE: [u8; 6] = [0, 1, b'x', 0, 0, 0xff];

fn source() -> Source {
    Source::from_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

async fn pending(fixture: &Fixture, id: [u8; 32], source: Source, privileged: bool) -> Vec<u8> {
    let salt = [4; 16];
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
            salt,
            stored_key: StoredKey(sha256(&CLIENT_KEY)),
            server_key: Zeroizing::new(SERVER_KEY),
            kdf_params: params,
            tickets_invalid_before_ns: 0,
        },
    )
    .expect("insert test user");
    if privileged {
        assert!(users.set_superuser(USER, true, 1).expect("mark privileged"));
    }
    let transcript = build_transcript(&HttpLoginTranscriptInput {
        protocol_version: 1,
        binding_mode: 3,
        security: fixture.context.security_class,
        listener_id: &fixture.context.listener_id,
        handshake_id: &id,
        user: USER,
        client_nonce: &[1; 32],
        server_nonce: &[2; 32],
        salt: &salt,
        memory_kb: params.memory_kb,
        time: params.time,
        parallelism: params.parallelism,
        argon2_version: params.argon2_version,
        scope_bytes: &SCOPE,
    })
    .expect("build retained transcript");
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
                source,
            },
        )
        .await
        .expect("insert pending entry");
    transcript
}

fn finish_body(id: [u8; 32], proof: [u8; 32]) -> Vec<u8> {
    let mut body = vec![0x83];
    pair(&mut body, "protocol_version", &[1]);
    pair(&mut body, "handshake_id", &binary(&id));
    pair(&mut body, "client_proof", &binary(&proof));
    body
}

fn binary(bytes: &[u8]) -> Vec<u8> {
    let mut output = vec![0xc4, u8::try_from(bytes.len()).expect("short binary")];
    output.extend_from_slice(bytes);
    output
}

fn pair(body: &mut Vec<u8>, key: &str, value: &[u8]) {
    body.push(0xa0 | u8::try_from(key.len()).expect("short key"));
    body.extend_from_slice(key.as_bytes());
    body.extend_from_slice(value);
}

fn proof(transcript: &[u8]) -> [u8; 32] {
    let signature = hmac_sha256(&sha256(&CLIENT_KEY), transcript);
    core::array::from_fn(|index| CLIENT_KEY[index] ^ signature[index])
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
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

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test(start_paused = true)]
async fn finish_tracing_redacts_credentials_and_secrets() {
    let fixture = Fixture::new().await;
    let valid_id = [0x91; 32];
    let valid_transcript = pending(&fixture, valid_id, source(), false).await;
    let valid_proof = proof(&valid_transcript);
    let wrong_id = [0x72; 32];
    let wrong_fixture = Fixture::new().await;
    let wrong_transcript = pending(&wrong_fixture, wrong_id, source(), false).await;
    let mut wrong_proof = proof(&wrong_transcript);
    wrong_proof[0] ^= 1;

    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(capture.clone())
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let issued_token = async move {
        tracing::info!("http login redaction sentinel");
        let valid_response = finish(
            &fixture.context,
            source(),
            &finish_body(valid_id, valid_proof),
        )
        .await;
        assert_eq!(valid_response.status(), StatusCode::OK);
        let decoded: SuccessBody =
            rmp_serde::from_slice(valid_response.body()).expect("decode response fields");
        let issued_token = decoded.token;

        let wrong_response = finish(
            &wrong_fixture.context,
            source(),
            &finish_body(wrong_id, wrong_proof),
        )
        .await;
        assert_eq!(wrong_response.status(), StatusCode::UNAUTHORIZED);
        issued_token
    }
    .with_subscriber(dispatch)
    .await;

    let logs =
        String::from_utf8(capture.0.lock().expect("capture lock").clone()).expect("UTF-8 logs");
    assert!(
        logs.contains("http login redaction sentinel"),
        "capture must include sentinel"
    );
    let secrets = [
        USER.to_owned(),
        source().address().to_string(),
        hex::encode(valid_id),
        hex::encode(wrong_id),
        hex::encode(valid_proof),
        hex::encode(wrong_proof),
        issued_token.clone(),
        issued_token
            .rsplit_once('.')
            .expect("token secret segment")
            .1
            .to_owned(),
        hex::encode([2; 32]),
    ];
    for secret in secrets {
        assert!(!logs.contains(&secret), "login secret leaked to trace");
    }
}

#[derive(serde::Serialize)]
struct BeginBody<'a> {
    protocol_version: u8,
    user: &'a str,
    client_nonce: &'a serde_bytes::Bytes,
    binding_mode: u8,
    token: BeginScope<'a>,
}

#[derive(serde::Serialize)]
struct BeginScope<'a> {
    label: &'a str,
    mode: &'a str,
    channels: &'a str,
    databases: Option<Vec<&'a str>>,
}

#[derive(serde::Deserialize)]
struct ChallengeId {
    handshake_id: serde_bytes::ByteBuf,
}

#[tokio::test(start_paused = true)]
async fn mathematically_valid_proof_for_ineligible_entry_never_issues() {
    let fixture = Fixture::new().await;
    let id = [16; 32];
    let transcript = pending(&fixture, id, source(), false).await;
    let pair = fixture.context.pair_key(USER, source().address());
    let mut entry = fixture
        .context
        .pending
        .claim(
            PendingId(id),
            source(),
            fixture.context.listener_id,
            fixture.context.security_class,
        )
        .await
        .expect("pending claim")
        .take()
        .expect("pending entry");
    entry.eligible = false;
    fixture
        .context
        .pending
        .insert(PendingId(id), entry)
        .await
        .expect("restore ineligible entry");
    let response = finish(
        &fixture.context,
        source(),
        &finish_body(id, proof(&transcript)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.body(),
        wire_response(401, "authentication_failed", None)
            .expect("canonical unauthorized response")
            .body()
    );
    assert_eq!(fixture.lockout.failure_pair_count(), 1);
    let status = fixture.lockout.pair_status(pair, 90_000_000_000);
    assert_eq!(status.last_fail_at_ns, Some(90_000_000_000));
    assert_eq!(
        fixture.lockout.current_backoff_ms(pair, 90_000_000_000),
        100
    );
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 0);
    assert_eq!(fixture.context.pending.count().await, 0);
    assert_eq!(
        finish(
            &fixture.context,
            source(),
            &finish_body(id, proof(&transcript))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(fixture.lockout.failure_pair_count(), 1);
    assert_eq!(
        fixture
            .lockout
            .pair_status(pair, 90_000_000_000)
            .last_fail_at_ns,
        Some(90_000_000_000)
    );
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 0);
}

#[tokio::test(start_paused = true)]
async fn hardened_mismatched_default_begin_finish_records_failure() {
    let mut fixture = Fixture::new().await;
    fixture.context.hardening = true;
    fixture.context.default_kdf.memory_kb = 19_457;
    let request = BeginBody {
        protocol_version: 1,
        user: "alice",
        client_nonce: serde_bytes::Bytes::new(&[3; 32]),
        binding_mode: 3,
        token: BeginScope {
            label: "http-login",
            mode: "read_only",
            channels: "rest",
            databases: None,
        },
    };
    let begin_body = rmp_serde::to_vec_named(&request).expect("serialize begin request");
    let begun = crate::http_login::service::begin(&fixture.context, source(), &begin_body).await;
    assert_eq!(begun.status(), StatusCode::OK);
    let challenge: ChallengeId = rmp_serde::from_slice(begun.body()).expect("challenge ID");
    let id: [u8; 32] = challenge
        .handshake_id
        .as_ref()
        .try_into()
        .expect("32-byte handshake ID");
    assert_eq!(fixture.context.pending.count().await, 1);

    let pair = fixture.context.pair_key("alice", source().address());
    let response = finish(&fixture.context, source(), &finish_body(id, [0; 32])).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.body(),
        wire_response(401, "authentication_failed", None)
            .expect("canonical unauthorized response")
            .body()
    );
    assert_eq!(
        fixture.lockout.current_backoff_ms(pair, 90_000_000_000),
        100
    );
    assert_eq!(
        fixture
            .lockout
            .pair_status(pair, 90_000_000_000)
            .last_fail_at_ns,
        Some(90_000_000_000)
    );
    assert_eq!(fixture.lockout.failure_pair_count(), 1);
    assert_eq!(fixture.context.pending.count().await, 0);
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 0);

    assert_eq!(
        finish(&fixture.context, source(), &finish_body(id, [0; 32]))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(fixture.lockout.failure_pair_count(), 1);
    assert_eq!(
        fixture.lockout.current_backoff_ms(pair, 90_000_000_000),
        100
    );
    assert_eq!(
        fixture
            .lockout
            .pair_status(pair, 90_000_000_000)
            .last_fail_at_ns,
        Some(90_000_000_000)
    );
    assert_eq!(fixture.context.pending.count().await, 0);
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 0);
}

#[tokio::test(start_paused = true)]
async fn finish_issues_token_and_returns_server_signature() {
    let fixture = Fixture::new().await;
    let id = [9; 32];
    let transcript = pending(&fixture, id, source(), false).await;
    let response = finish(
        &fixture.context,
        source(),
        &finish_body(id, proof(&transcript)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let decoded: SuccessBody =
        rmp_serde::from_slice(response.body()).expect("decode response fields");
    assert_eq!(decoded.protocol_version, 1);
    assert_eq!(
        decoded.server_signature.as_ref(),
        build_server_signature(&SERVER_KEY, &transcript).as_slice()
    );
    assert_eq!(
        (decoded.owner.as_str(), decoded.label.as_str()),
        (USER, "x")
    );
    assert_eq!(decoded.mode, "read_only");
    assert_eq!(decoded.channels, "rest");
    assert_eq!(decoded.databases, None);
    assert!(!decoded.token.is_empty());
    let parsed_id = parse_token(decoded.token.as_bytes()).expect("issued token parses");
    assert_eq!(parsed_id.id.0.as_slice(), decoded.id.as_ref());
    assert_eq!(decoded.id.len(), 16);
    assert!(decoded.expires_at_ns - decoded.created_at_ns <= 86_400_000_000_000);
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 1);
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SuccessBody {
    protocol_version: u8,
    server_signature: serde_bytes::ByteBuf,
    token: String,
    id: serde_bytes::ByteBuf,
    label: String,
    owner: String,
    mode: String,
    channels: String,
    databases: Option<Vec<String>>,
    created_at_ns: u64,
    expires_at_ns: u64,
}

#[tokio::test(start_paused = true)]
async fn incorrect_proof_records_one_pair_failure_and_consumes_claim() {
    let fixture = Fixture::new().await;
    let id = [10; 32];
    let transcript = pending(&fixture, id, source(), false).await;
    let mut bad = proof(&transcript);
    bad[0] ^= 1;
    assert_eq!(
        finish(&fixture.context, source(), &finish_body(id, bad))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(fixture.lockout.failure_pair_count(), 1);
    assert_eq!(
        finish(
            &fixture.context,
            source(),
            &finish_body(id, proof(&transcript))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(fixture.lockout.failure_pair_count(), 1);
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 0);
}

#[tokio::test(start_paused = true)]
async fn wrong_source_does_not_consume_correct_claim() {
    let fixture = Fixture::new().await;
    let id = [11; 32];
    let right = source();
    let transcript = pending(&fixture, id, right, false).await;
    let wrong = Source::from_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)));
    assert_eq!(
        finish(
            &fixture.context,
            wrong,
            &finish_body(id, proof(&transcript))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(fixture.lockout.failure_pair_count(), 0);
    assert_eq!(
        finish(
            &fixture.context,
            right,
            &finish_body(id, proof(&transcript))
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test(start_paused = true)]
async fn pair_failure_after_begin_skips_proof_without_accounting_again() {
    let fixture = Fixture::new().await;
    let id = [13; 32];
    let transcript = pending(&fixture, id, source(), false).await;
    let pair = fixture.context.pair_key(USER, source().address());
    fixture.lockout.register_failure(pair, 90_000_000_000);
    let before = fixture.lockout.pair_status(pair, 90_000_000_000);
    let backoff = fixture.lockout.current_backoff_ms(pair, 90_000_000_000);
    assert_eq!(
        finish(
            &fixture.context,
            source(),
            &finish_body(id, proof(&transcript))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let after = fixture.lockout.pair_status(pair, 90_000_000_000);
    assert_eq!(after.last_fail_at_ns, before.last_fail_at_ns);
    assert_eq!(
        fixture.lockout.current_backoff_ms(pair, 90_000_000_000),
        backoff
    );
    assert_eq!(fixture.lockout.failure_pair_count(), 1);
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 0);
}

#[tokio::test(start_paused = true)]
async fn user_floor_change_before_finish_rejects_valid_proof() {
    let fixture = Fixture::new().await;
    let id = [14; 32];
    let transcript = pending(&fixture, id, source(), false).await;
    let users = Arc::clone(&fixture.context.users);
    assert!(users
        .bump_tickets_invalid(USER, 90_000_000_000)
        .expect("raise ticket floor"));
    assert_eq!(
        finish(
            &fixture.context,
            source(),
            &finish_body(id, proof(&transcript))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(fixture.lockout.failure_pair_count(), 0);
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 0);
}

#[tokio::test(start_paused = true)]
async fn disabled_self_service_denies_valid_proof_without_pair_failure() {
    let mut fixture = Fixture::new().await;
    let id = [15; 32];
    let transcript = pending(&fixture, id, source(), false).await;
    fixture.context.self_service = false;
    assert_eq!(
        finish(
            &fixture.context,
            source(),
            &finish_body(id, proof(&transcript))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(fixture.lockout.failure_pair_count(), 0);
    assert_eq!(fixture.context.pending.count().await, 0);
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 0);
}

#[tokio::test(start_paused = true)]
async fn valid_proof_for_privileged_user_is_denied_without_pair_failure() {
    let fixture = Fixture::new().await;
    let id = [12; 32];
    let transcript = pending(&fixture, id, source(), true).await;
    assert_eq!(
        finish(
            &fixture.context,
            source(),
            &finish_body(id, proof(&transcript))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(fixture.lockout.failure_pair_count(), 0);
    assert_eq!(fixture.context.tokens.mirror_entry_count_hint(), 0);
}
