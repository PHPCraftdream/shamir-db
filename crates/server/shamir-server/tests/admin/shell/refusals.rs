//! R5 PR2 transport policy and pre-dial token-subset acceptance.

use std::net::SocketAddr;
use std::time::Duration;

use serde_json::Value;
use shamir_query_builder::token_admin;
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::harness::{
    overwrite_profile, profile_text, run_shell, seed_catalogue, seed_shell_user, ShellFixture,
    ShellRun, DB_NAME, PROFILE_NAME, REPO_NAME, SHELL_USER,
};
use super::secure_fixture::{boot_secure, provision_token, run_token_shell, write_secure_profile};

const BOUND: Duration = Duration::from_secs(120);
const PLAINTEXT: &str = "refusing plaintext admin connection; use TLS TCP, WSS, or local IPC";
const TOKEN_TRANSPORT: &str = "API tokens require HTTPS REST or the native token WSS listener";
const DEFERRED: &str =
    "browser profile and HTTP password login are deferred; HTTP login mints a reusable API token, not a session";
const RESUME: &str = "session resume is never used by the admin shell; log in fresh";
const MISSING_PIN: &str =
    "SCRAM server identity pin is required; configure a verified pin in the profile";
const PIN_MISMATCH: &str = "server identity pin does not match the verified pin in the profile";
const TOKEN_SUBSET: &str =
    "token sessions expose only the read-only shell subset; log in with SCRAM for this command";

fn record(run: &ShellRun, command: &str) -> Value {
    let line = run
        .stdout
        .strip_suffix(b"\n")
        .expect("LF-terminated output");
    assert!(!line.contains(&b'\n'), "expected exactly one record");
    let value: Value =
        serde_json::from_slice(line).unwrap_or_else(|_| panic!("invalid JSON output"));
    assert_eq!(value["version"].as_u64(), Some(1), "wrong version");
    assert!(
        value["command"].as_str() == Some(command),
        "wrong command ID"
    );
    assert!(
        value.get("node").is_some_and(Value::is_null),
        "unexpected node"
    );
    assert!(run.stderr.is_empty(), "unexpected batch diagnostics");
    value
}

fn assert_refusal(
    run: &ShellRun,
    command: &str,
    op: Option<&str>,
    exit: i32,
    code: &str,
    message: &str,
) {
    assert_eq!(run.exit_code, exit, "wrong exit for {command} ({code})");
    let value = record(run, command);
    assert!(value["kind"].as_str() == Some("error"), "expected error");
    assert!(
        value["code"].as_str() == Some(code),
        "wrong code for {command}"
    );
    assert!(
        value["message"].as_str() == Some(message),
        "wrong message for {code}"
    );
    assert!(
        value.get("data").is_some_and(Value::is_null),
        "unexpected data"
    );
    assert!(
        value.get("op").is_some_and(|value| match op {
            Some(op) => value.as_str() == Some(op),
            None => value.is_null(),
        }),
        "wrong op for {command} ({code})"
    );
}

fn policy_probe(
    fixture: &ShellFixture,
    args: &[&str],
    command: &str,
    op: Option<&str>,
    expected: (&str, &str),
) {
    let run = run_shell(fixture, args);
    assert!(
        !run.contains_canary(fixture.canary.as_str()),
        "shell secret leaked"
    );
    assert_refusal(&run, command, op, 6, expected.0, expected.1);
}

#[test]
fn shell_secure_transport_refusals() {
    let secure = boot_secure("shell_secure_transport_refusals");
    let fixture = &secure.shell;
    let profile = |transport: &str, address: &str| {
        let sni = (transport != "ipc").then_some("localhost");
        profile_text(transport, address, sni, Some(DB_NAME))
    };
    let write = |transport: &str, address: &str| {
        let text = profile(transport, address)
            .replace("pin: <PIN>\n", &format!("pin: {}\n", fixture.pin_hex()));
        overwrite_profile(fixture, &text);
    };
    let loopback = fixture.tls_addr.to_string();
    for transport in ["plain", "http", "ws"] {
        write(transport, &loopback);
        policy_probe(
            fixture,
            &["ping"],
            "ping",
            Some("Ping"),
            ("plaintext_refused", PLAINTEXT),
        );
        policy_probe(
            fixture,
            &["--auth", "token", "--", "ping"],
            "ping",
            Some("Ping"),
            ("token_transport_refused", TOKEN_TRANSPORT),
        );
    }
    for (transport, addr) in [
        ("tls-tcp", loopback.as_str()),
        ("ipc", fixture.ipc_addr.as_str()),
    ] {
        write(transport, addr);
        policy_probe(
            fixture,
            &["--auth", "token", "--", "ping"],
            "ping",
            Some("Ping"),
            ("token_transport_refused", TOKEN_TRANSPORT),
        );
    }
    for transport in ["browser-wss", "browser-token", "http-login", "tls-resume"] {
        write(transport, &loopback);
        let expected = if transport == "tls-resume" {
            ("resume_refused", RESUME)
        } else {
            ("deferred", DEFERRED)
        };
        policy_probe(fixture, &["ping"], "ping", Some("Ping"), expected);
        policy_probe(fixture, &["connect"], "connect", None, expected);
    }

    write("tls-tcp", &loopback);
    for auth in ["browser", "http-login"] {
        let expected = ("deferred", DEFERRED);
        policy_probe(
            fixture,
            &["--auth", auth, "--", "ping"],
            "ping",
            None,
            expected,
        );
        policy_probe(
            fixture,
            &["--auth", auth, "--", "connect"],
            "connect",
            None,
            expected,
        );
        policy_probe(
            fixture,
            &["connect", "--auth", auth],
            "connect",
            None,
            expected,
        );
    }
    let expected = ("resume_refused", RESUME);
    policy_probe(fixture, &["--resume", "--", "ping"], "ping", None, expected);
    policy_probe(
        fixture,
        &["connect", "--auth", "resume"],
        "connect",
        None,
        expected,
    );

    // Both native WSS spellings must reach the pin policy, not the old
    // PR1 unsupported-transport branch. A mismatched pin uses the live WSS peer.
    let wss = secure.wss_addr.to_string();
    overwrite_profile(
        fixture,
        &profile("native-wss", &wss).replace("pin: <PIN>\n", ""),
    );
    policy_probe(
        fixture,
        &["ping"],
        "ping",
        Some("Ping"),
        ("missing_pin", MISSING_PIN),
    );
    let wrong_pin = if fixture.pin == [0; 32] { "ab" } else { "00" }.repeat(32);
    let text = profile("wss", &wss).replace("pin: <PIN>\n", &format!("pin: {wrong_pin}\n"));
    overwrite_profile(fixture, &text);
    policy_probe(
        fixture,
        &["ping"],
        "ping",
        Some("Ping"),
        ("pin_mismatch", PIN_MISMATCH),
    );
}

/// One subprocess owns a fresh relay. The relay counts the first downstream
/// connection (0 or 1 is enough to falsify pre-dial), forwarding TLS unchanged.
struct Relay {
    addr: SocketAddr,
    stop: oneshot::Sender<()>,
    task: JoinHandle<usize>,
}

impl Relay {
    fn start(fixture: &ShellFixture, target: SocketAddr) -> Self {
        fixture.rt.run(async move {
            let listener = timeout(BOUND, TcpListener::bind("127.0.0.1:0"))
                .await
                .expect("relay bind timed out")
                .expect("relay bind failed");
            let addr = listener.local_addr().expect("relay bound address");
            let (stop, mut stopped) = oneshot::channel();
            let task = tokio::spawn(async move {
                timeout(BOUND, async {
                    let accepted = tokio::select! {
                        accepted = listener.accept() => Some(accepted),
                        _ = &mut stopped => None,
                    };
                    let Some(accepted) = accepted else {
                        // The child has exited. Check the kernel's accept
                        // queue directly, even if Tokio has not yet delivered
                        // the socket readiness event. into_std is nonblocking.
                        let listener = listener.into_std().expect("convert relay listener");
                        return match listener.accept() {
                            Ok(_) => 1,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => 0,
                            Err(_) => panic!("relay accept queue check failed"),
                        };
                    };
                    let mut downstream = accepted.expect("relay accept failed").0;
                    tokio::select! {
                        // Stop abandons the copy and drops both streams; the
                        // positive control has already received its reply.
                        _ = &mut stopped => {},
                        () = async {
                            let mut upstream = TcpStream::connect(target)
                                .await
                                .expect("relay upstream connect failed");
                            // A child can reset TCP during teardown. The shell
                            // result, not a clean TLS shutdown, is the control.
                            let _ = tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await;
                        } => {},
                    }
                    1
                })
                .await
                .expect("relay timed out")
            });
            Self { addr, stop, task }
        })
    }

    fn finish(self, fixture: &ShellFixture) -> usize {
        // The receiver can already be gone if the connected child closed TCP.
        let _ = self.stop.send(());
        fixture.rt.run(async move {
            let mut task = self.task;
            match timeout(BOUND, &mut task).await {
                Ok(result) => result.expect("relay task failed"),
                Err(_) => {
                    task.abort();
                    panic!("relay join timed out");
                }
            }
        })
    }
}

#[test]
fn shell_token_commands_outside_subset_never_dial() {
    let secure = boot_secure("shell_token_commands_outside_subset_never_dial");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    for transport in ["https", "token-wss"] {
        fixture.guard.mark(transport);
        let token = provision_token(
            fixture,
            token_admin::create_api_token(transport)
                .for_user(SHELL_USER)
                .mode(ApiTokenMode::ReadOnly)
                .channels(ApiTokenChannels::Both),
        );
        let probe = |args: &[&str]| {
            let relay = Relay::start(fixture, secure.http_addr);
            write_secure_profile(
                fixture,
                transport,
                relay.addr,
                DB_NAME,
                Some(&secure.ca_path),
            );
            let run = run_token_shell(fixture, &token, args);
            let accepted = relay.finish(fixture);
            (run, accepted)
        };
        // Validate the same TLS target, profile shape and token first. An
        // accidental dial followed by a local denial still increments the relay.
        let (positive, accepted) = probe(&["ping"]);
        assert_eq!(positive.exit_code, 0, "{transport} positive control failed");
        let value = record(&positive, "ping");
        assert!(value["kind"].as_str() == Some("result"));
        assert!(value["op"].as_str() == Some("Ping"));
        assert_eq!(value["data"]["ok"].as_bool(), Some(true));
        assert_eq!(accepted, 1, "{transport} positive control missed relay");

        // All parseable PR2 commands outside the positive subset; connect is
        // lifecycle, and the later-stage command grammar is deliberately absent.
        for (args, command) in [
            (&["repo", "describe", REPO_NAME][..], "repo describe"),
            (&["profile", "list"][..], "profile list"),
            (&["profile", "show", PROFILE_NAME][..], "profile show"),
        ] {
            let (run, accepted) = probe(args);
            assert_eq!(accepted, 0, "{transport} {command} dialed before refusal");
            assert_refusal(&run, command, None, 4, "token_request_denied", TOKEN_SUBSET);
        }
    }
}
