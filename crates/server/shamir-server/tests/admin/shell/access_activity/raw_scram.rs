use tokio::io::{split, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio_rustls::{client::TlsStream, TlsConnector};

use shamir_connect::client::handshake::{HandshakeBuilder, ServerAuthOk, ServerChallenge};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;
use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_transport_tcp::tls::{
    extract_tls_exporter, make_client_config_app_authenticated, AppLayerServerAuth,
};

use super::harness::{ShellFixture, ShellRun, ADMIN, SHELL_USER};
use super::secure_fixture::run_secret_process_timeout_scanned;
use super::state_data_common::data;
use serde_json::Value;
use shamir_server::db_handler::{DbRequest, DbResponse};
use std::time::{Duration, Instant};

/// Fields of one projected request row.
pub(super) const REQUEST_FIELDS: &[&str] = &[
    "kind",
    "request_exec_id",
    "session_handle",
    "user",
    "db",
    "op_kind",
    "state",
    "elapsed_ns",
];

/// Documented request-state renderings.
pub(super) const REQUEST_STATES: &[&str] = &[
    "running",
    "done",
    "committing",
    "cancelled",
    "waiting_lock",
    "waiting_visibility",
    "waiting_budget",
];

/// Exactly the top-level keys of `value`.
pub(super) fn keys_exact(value: &Value, expected: &[&str], context: &str) {
    let fields = value
        .as_object()
        .unwrap_or_else(|| panic!("{context} is not a JSON object"));
    let mut actual = fields.keys().map(String::as_str).collect::<Vec<_>>();
    actual.sort_unstable();
    let mut wanted = expected.to_vec();
    wanted.sort_unstable();
    assert_eq!(actual, wanted, "{context} field set");
}

/// Read one documented decimal-string leaf without echoing its value.
pub(super) fn decimal(value: &Value, context: &str) -> String {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("{context} is not a decimal string"));
    assert!(
        !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()),
        "{context} is not a decimal string"
    );
    text.to_owned()
}

/// Prove `parked_handle` belongs to A's sessions and return this shell's
/// admin handle (the newest monotonically minted one on the page).
pub(super) fn shell_own_handle(
    fixture: &ShellFixture,
    needles: &[&str],
    parked_handle: &str,
    deadline: Instant,
) -> String {
    let cap = deadline
        .saturating_duration_since(Instant::now())
        .min(Duration::from_secs(10));
    assert!(!cap.is_zero(), "active deadline expired before shell child");
    scanner_positive_control();
    let run = run_secret_process_timeout_scanned(
        fixture,
        Some(fixture.canary.clone()),
        false,
        None,
        &["activity", "sessions", "--all"],
        cap,
        (&[], needles),
    );
    scan_streams(&run, needles);
    let payload = data(&run, "sessions");
    assert_eq!(payload["scope"], "all", "`--all` must widen the scope");
    let rows = payload["rows"]
        .as_array()
        .expect("activity sessions `rows` is not an array");
    let foreign_handles: Vec<String> = rows
        .iter()
        .filter(|row| row["user"].as_str() == Some(SHELL_USER))
        .map(|row| decimal(&row["session_handle"], "session A handle"))
        .collect();
    assert_eq!(
        foreign_handles.len(),
        2,
        "the page must carry exactly A's two live sessions"
    );
    assert!(
        foreign_handles.iter().any(|handle| handle == parked_handle),
        "the parked request row belongs to one of A's live sessions"
    );
    // Handles are minted monotonically, so the listing shell's own (newest)
    // admin session is the maximum admin handle on the page. Earlier batch
    // invocations may still be draining and must not be mistaken for it.
    let shell_row = rows
        .iter()
        .filter(|row| row["user"].as_str() == Some(ADMIN))
        .max_by_key(|row| {
            decimal(&row["session_handle"], "admin session_handle")
                .parse::<u64>()
                .expect("decimal session handle fits u64")
        })
        .expect("the listing shell's own admin session must be on the page");
    let shell_handle = decimal(&shell_row["session_handle"], "shell session_handle");
    for handle in &foreign_handles {
        assert_ne!(
            handle, &shell_handle,
            "A and the shell are distinct sessions"
        );
    }
    shell_handle
}

#[derive(serde::Deserialize)]
struct ChallengeWire {
    #[serde(with = "serde_bytes")]
    salt: Vec<u8>,
    memory_kb: u32,
    time: u32,
    parallelism: u32,
    argon2_version: u8,
    #[serde(with = "serde_bytes")]
    server_nonce: Vec<u8>,
}

#[derive(serde::Deserialize)]
struct AuthOkWire {
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
    // The wire frame is a positional array: the trailing fields must be
    // declared even when unused, or the decode fails with `LengthMismatch`.
    #[serde(default)]
    _server_query_version: u8,
    #[serde(default)]
    _rotation_in_progress: Option<shamir_connect::common::wire::RotationInProgressWire>,
    #[serde(default)]
    _kdf_upgrade_required: Option<bool>,
}

#[derive(serde::Serialize)]
struct AuthInitWire {
    user: String,
    #[serde(with = "serde_bytes")]
    client_nonce: Vec<u8>,
    binding_mode: u8,
    version: u8,
}

#[derive(serde::Serialize)]
struct ClientProofWire {
    #[serde(with = "serde_bytes")]
    client_proof: Vec<u8>,
}

pub(super) fn bytes_contain(haystack: &[u8], needle: &str) -> bool {
    needle.len() <= haystack.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
}

pub(super) fn scan_streams(run: &ShellRun, needles: &[&str]) {
    for needle in needles {
        assert!(
            !bytes_contain(&run.stdout, needle) && !bytes_contain(&run.stderr, needle),
            "session material leaked in a shell byte stream"
        );
    }
}

/// Owned-slice control proving the byte matcher is live before scans.
pub(super) fn scanner_positive_control() {
    let probe = "parked-cancel-scanner-positive-control";
    let owned = format!("prefix:{probe}:suffix").into_bytes();
    assert!(
        bytes_contain(&owned, probe),
        "scanner positive control failed"
    );
}

/// Run the real shell and reject session material in captured streams.
pub(super) fn checked_shell_timeout(
    fixture: &ShellFixture,
    needles: &[&str],
    args: &[&str],
    timeout: Duration,
) -> ShellRun {
    scanner_positive_control();
    let run = run_secret_process_timeout_scanned(
        fixture,
        Some(fixture.canary.clone()),
        false,
        None,
        args,
        timeout,
        (&[], needles),
    );
    scan_streams(&run, needles);
    run
}

/// Ensure no entry name or file bytes under the profile home carry secrets.
pub(super) fn scan_profile_home(fixture: &ShellFixture, needles: &[&str]) {
    let mut dirs = vec![fixture.profile_home.path().to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("read profile home") {
            let entry = entry.expect("profile home entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            for needle in needles {
                assert!(
                    !name.contains(needle),
                    "session material in a profile-home entry name"
                );
            }
            let kind = entry.file_type().expect("profile home entry type");
            if kind.is_dir() {
                dirs.push(entry.path());
                continue;
            }
            assert!(kind.is_file(), "unexpected non-file in the profile home");
            let bytes = std::fs::read(entry.path()).expect("read profile home file");
            for needle in needles {
                assert!(
                    !bytes_contain(&bytes, needle),
                    "session material in a profile-home file"
                );
            }
        }
    }
}
pub(super) async fn login(
    addr: std::net::SocketAddr,
    username: &str,
    password: &[u8],
    trusted_pin: [u8; 32],
) -> (
    [u8; 32],
    ReadHalf<TlsStream<TcpStream>>,
    WriteHalf<TlsStream<TcpStream>>,
    u32,
) {
    let norm_username = NormalizedUsername::from_raw(username).expect("username");
    let config = make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound())
        .expect("client TLS config");
    let connector = TlsConnector::from(config);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tcp = TcpStream::connect(addr).await.expect("TCP connect");
    let tls = connector
        .connect(server_name, tcp)
        .await
        .expect("TLS handshake");
    let exporter = extract_tls_exporter(&tls).expect("client exporter");
    let handshake =
        HandshakeBuilder::new(norm_username, TransportKind::Tcp, BindingMode::TlsExporter)
            .tls_exporter(exporter)
            .pinned_hash(trusted_pin)
            .build()
            .expect("pinned handshake builder");
    let (mut read, mut write) = split(tls);

    let init = handshake.auth_init();
    let wire = AuthInitWire {
        user: init.user,
        client_nonce: init.client_nonce.to_vec(),
        binding_mode: init.binding_mode,
        version: init.version,
    };
    write_frame(
        &mut write,
        &rmp_serde::to_vec(&wire).expect("encode auth_init"),
    )
    .await
    .expect("send auth_init");
    let bytes = read_frame(&mut read, MAX_FRAME_SIZE_DEFAULT)
        .await
        .expect("read challenge");
    let wire: ChallengeWire = rmp_serde::from_slice(&bytes).expect("decode challenge");
    let mut salt = [0; 16];
    salt.copy_from_slice(&wire.salt);
    let mut server_nonce = [0; 32];
    server_nonce.copy_from_slice(&wire.server_nonce);
    let challenge = ServerChallenge {
        salt,
        kdf_params: KdfParams {
            memory_kb: wire.memory_kb,
            time: wire.time,
            parallelism: wire.parallelism,
            argon2_version: wire.argon2_version,
        },
        server_nonce,
    };
    let mut password = password.to_vec();
    let (proof, derived, auth_message) = handshake
        .process_challenge(&challenge, &mut password)
        .expect("process challenge");
    let proof = ClientProofWire {
        client_proof: proof.to_vec(),
    };
    write_frame(
        &mut write,
        &rmp_serde::to_vec(&proof).expect("encode proof"),
    )
    .await
    .expect("send proof");

    let bytes = read_frame(&mut read, MAX_FRAME_SIZE_DEFAULT)
        .await
        .expect("read auth_ok");
    let wire: AuthOkWire = rmp_serde::from_slice(&bytes).expect("decode auth_ok");
    let mut server_signature = [0; 32];
    server_signature.copy_from_slice(&wire.server_signature);
    let mut server_pub_key = [0; 32];
    server_pub_key.copy_from_slice(&wire.server_pub_key);
    let mut identity_sig = [0; 64];
    identity_sig.copy_from_slice(&wire.identity_sig);
    let mut session_id = [0; 32];
    session_id.copy_from_slice(&wire.session_id);
    let auth_ok = ServerAuthOk {
        server_signature,
        server_pub_key,
        identity_sig,
        session_id,
        expires_at_ns: wire.expires_at_ns,
        resumption_ticket: Some(wire.resumption_ticket),
        resumption_expires_at_ns: Some(wire.resumption_expires_at_ns),
        rotation_in_progress: None,
        kdf_upgrade_required: None,
    };
    handshake
        .process_auth_ok(&auth_ok, &derived, &auth_message, |_| {})
        .expect("verify SCRAM signature, fixture pin, and server identity signature");
    (session_id, read, write, 1)
}

pub(super) async fn roundtrip<R, W>(
    request: &DbRequest,
    session_id: [u8; 32],
    next_request_id: &mut u32,
    write: &mut W,
    read: &mut R,
) -> DbResponse
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    roundtrip_inner(request, session_id, next_request_id, write, read).await
}

async fn roundtrip_inner<R, W>(
    request: &DbRequest,
    session_id: [u8; 32],
    next_request_id: &mut u32,
    write: &mut W,
    read: &mut R,
) -> DbResponse
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let request_id = *next_request_id;
    *next_request_id += 1;
    let payload = rmp_serde::to_vec_named(request).expect("encode request");
    let envelope = RequestEnvelope::new(session_id, Some(request_id), payload)
        .to_msgpack()
        .expect("encode request envelope");
    write_frame(write, &envelope).await.expect("send request");
    let bytes = read_frame(read, MAX_FRAME_SIZE_DEFAULT)
        .await
        .expect("read response");
    let envelope = ResponseEnvelope::from_msgpack(&bytes).expect("decode response envelope");
    assert_eq!(envelope.request_id, Some(request_id), "request id echoed");
    rmp_serde::from_slice(&envelope.res).expect("decode response")
}
