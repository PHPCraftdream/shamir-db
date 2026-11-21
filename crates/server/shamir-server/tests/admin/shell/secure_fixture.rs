//! Secure-listener extension, typed ACL/token setup, and shared child runner.

use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;

use shamir_db::access::{Actor, ResourceMeta, ResourcePath};
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::token_admin::CreateApiToken;
use shamir_query_types::admin::ResourceRef;
use shamir_query_types::hmac as canon;
use shamir_server::config::HttpListenerSettings;
use shamir_server::config::ListenerConfig;
use shamir_server::config::ListenerKind;
use shamir_server::config::ProfileKind;
use shamir_sync::format_stamp::{check_data_dir, EpochPolicy, IgnoreSet, StampMode};
use tokio::io::AsyncWriteExt;
use zeroize::Zeroizing;

use super::boot_with;
use super::bound;
use super::connect_tls_owned;
use super::contains;
use super::overwrite_profile;
use super::profile_env;
use super::profile_home_path;
use super::server_command;
use super::ShellFixture;
use super::ShellRun;
use super::TestRuntime;
use super::ADMIN;
use super::BOOT_BOUND;
use super::PROFILE_NAME;
use super::ROUTING_DB;
use super::SHELL_BOUND;

pub(in crate::shell_tests) struct SecureFixture {
    pub(in crate::shell_tests) shell: ShellFixture,
    pub(in crate::shell_tests) wss_addr: SocketAddr,
    pub(in crate::shell_tests) http_addr: SocketAddr,
    pub(in crate::shell_tests) ca_path: PathBuf,
}

/// Boot WSS SCRAM and HTTPS REST/token WSS alongside the PR1 listeners.
pub(in crate::shell_tests) fn boot_secure(name: &str) -> SecureFixture {
    boot_secure_with(name, false)
}

pub(in crate::shell_tests) fn boot_secure_root_list(name: &str) -> SecureFixture {
    boot_secure_with(name, true)
}

fn boot_secure_with(name: &str, root_list: bool) -> SecureFixture {
    let (shell, bound_addrs, cert_path) = boot_with(name, root_list, |config| {
        config.listeners.extend([
            ListenerConfig {
                kind: ListenerKind::Ws,
                addr: "127.0.0.1:0".into(),
                profile: Some(ProfileKind::TlsExporter),
                path: Some("/shamir/v1".into()),
                kdf_override: None,
                browser_origin_allowlist: vec![],
                http: Default::default(),
            },
            ListenerConfig {
                kind: ListenerKind::Http,
                addr: "127.0.0.1:0".into(),
                profile: None,
                path: None,
                kdf_override: None,
                browser_origin_allowlist: vec![],
                http: HttpListenerSettings {
                    tls: Some(true),
                    ..Default::default()
                },
            },
        ]);
    });
    let [.., Some(wss_addr), Some(http_addr)] = bound_addrs.as_slice() else {
        panic!("secure fixture listeners did not bind");
    };
    let ca_path = profile_home_path(&shell).join("fixture-ca.pem");
    // This copies only the public certificate. The child gets no path into
    // the server data directory, and verifies the certificate normally.
    std::fs::copy(cert_path, &ca_path).expect("copy fixture public certificate");
    shell.guard.note("wss_addr", wss_addr.to_string());
    shell.guard.note("http_addr", http_addr.to_string());
    SecureFixture {
        shell,
        wss_addr: *wss_addr,
        http_addr: *http_addr,
        ca_path,
    }
}

/// Root cannot be addressed by wire ACL operations. Seed it before launch
/// in this fixture's private directory, using the launcher's system path.
pub(super) fn seed_root_list(data_dir: &Path) {
    check_data_dir(
        data_dir,
        StampMode::Writer,
        &IgnoreSet::none(),
        &EpochPolicy::compiled(),
    )
    .expect("stamp empty Root List fixture directory");
    let system_path = data_dir.join("shamir_db_meta.redb");
    // Join the offline runtime before starting the launcher.
    TestRuntime::new().run(bound(BOOT_BOUND, async move {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path))
            .await
            .expect("open Root List fixture system store");
        db.set_resource_meta(
            &ResourcePath::Root,
            &ResourceMeta {
                owner: Actor::System,
                group: None,
                mode: 0o755,
            },
        )
        .await
        .expect("seed Root List ACL");
        db.flush_all_waiting().await.expect("flush Root List ACL");
    }));
}

#[derive(serde::Serialize)]
struct SecureProfile<'a> {
    transport: &'a str,
    address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<&'static str>,
    sni: &'static str,
    database: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ca_path: Option<&'a Path>,
}

/// Write a pinned SCRAM profile (`None`) or a token profile with a trusted PEM.
pub(in crate::shell_tests) fn write_secure_profile(
    fixture: &ShellFixture,
    transport: &str,
    addr: SocketAddr,
    database: &str,
    ca_path: Option<&Path>,
) -> PathBuf {
    let profile = SecureProfile {
        transport,
        address: addr.to_string(),
        username: ca_path.is_none().then_some(ADMIN),
        sni: "localhost",
        database,
        pin: ca_path.is_none().then(|| fixture.pin_hex()),
        ca_path,
    };
    let text = ktav::to_string(&profile).expect("serialize secure fixture profile");
    assert!(ktav::parse_strict(&text).is_ok(), "strict secure profile");
    overwrite_profile(fixture, &text)
}

/// Issue a real fresh token over the pinned admin SCRAM connection. The
/// caller chooses the owner, databases, mode and channels on `request`.
pub(in crate::shell_tests) fn provision_token(
    fixture: &ShellFixture,
    request: CreateApiToken,
) -> Zeroizing<String> {
    fixture.guard.mark("seed: API token");
    let addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    fixture.rt.run(bound(BOOT_BOUND, async move {
        let client = connect_tls_owned(addr, pin, password)
            .await
            .expect("verified admin connection for token issuance");
        // Do not format a response or issuance error that could carry a secret.
        let created = client
            .create_api_token(request)
            .await
            .unwrap_or_else(|_| panic!("fixture token issuance failed"));
        let secret = Zeroizing::new(created.token.reveal().to_owned());
        client.close().await;
        secret
    }))
}

/// Revoke an issued token by its public ID over pinned admin SCRAM.
pub(in crate::shell_tests) fn revoke_fixture_token(fixture: &ShellFixture, token_id: &str) {
    fixture.guard.mark("seed: revoke API token");
    let addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    let token_id = token_id.to_owned();
    fixture.rt.run(bound(BOOT_BOUND, async move {
        let client = connect_tls_owned(addr, pin, password)
            .await
            .expect("verified admin connection for token revocation");
        client
            .revoke_api_token(&token_id)
            .await
            .expect("revoke fixture token");
        client.close().await;
    }));
}

/// Set a resource's mode through an authenticated, signed typed operation.
/// The routing database must already exist; Root is not a wire resource.
pub(in crate::shell_tests) fn set_acl_mode(
    fixture: &ShellFixture,
    resource: ResourceRef,
    mode: u16,
) {
    fixture.guard.mark("seed: token catalogue ACL");
    let addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    fixture.rt.run(bound(BOOT_BOUND, async move {
        let client = connect_tls_owned(addr, pin, password)
            .await
            .expect("verified admin connection for ACL setup");
        let unsigned = ddl::chmod(resource.clone(), mode)
            .hmac("0".repeat(64))
            .build()
            .expect("typed chmod with placeholder tag");
        let key = canon::derive_session_hmac_key(&client.session_id());
        let bytes =
            canon::canonical_batch_op(ROUTING_DB, &unsigned).expect("chmod canonical bytes");
        let op = ddl::chmod(resource, mode)
            .hmac(canon::compute_tag_hex(&key, &bytes))
            .build()
            .expect("signed typed chmod");
        let mut batch = Batch::new();
        batch.id("shell_fixture_acl");
        batch.chmod("mode", op);
        let result = client
            .execute(ROUTING_DB, batch.build())
            .await
            .expect("apply ACL");
        let result = result.results.get("mode").expect("chmod result alias");
        assert_eq!(result.records.len(), 1, "chmod returned one record");
        assert_eq!(
            result.records[0].get_value_i64("mode"),
            Some(i64::from(mode))
        );
        client.close().await;
    }));
}

/// Run the token shell and scan both streams and the operator home for
/// the token (including its secret suffix) and for unexpected history.
pub(in crate::shell_tests) fn run_token_shell(
    fixture: &ShellFixture,
    token: &Zeroizing<String>,
    args: &[&str],
) -> ShellRun {
    assert!(!token.is_empty(), "fixture token must not be empty");
    let run = run_with_secret(fixture, token.clone(), true, args);
    let suffix = token_secret_suffix(token);
    assert!(
        !run.contains_canary(token) && !run.contains_canary(suffix),
        "token leaked in shell output"
    );
    let mut dirs = vec![profile_home_path(fixture)];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).expect("scan operator home") {
            let entry = entry.expect("operator home entry");
            let kind = entry.file_type().expect("operator home entry type");
            assert!(
                !entry
                    .file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("history"),
                "unexpected shell history"
            );
            if kind.is_dir() {
                dirs.push(entry.path());
            } else {
                assert!(kind.is_file(), "unexpected non-file in operator home");
                let bytes = std::fs::read(entry.path()).expect("read operator file");
                assert!(
                    !contains(&bytes, token) && !contains(&bytes, suffix),
                    "token leaked in operator home"
                );
            }
        }
    }
    run
}

fn token_secret_suffix(token: &str) -> &str {
    token.rsplit_once('.').map_or(
        token,
        |(_, tail)| if tail.is_empty() { token } else { tail },
    )
}

/// Run the real binary with a zeroizing, stdin-only secret. Arguments with
/// an explicit `--` are used as supplied; otherwise they are the command.
pub(in crate::shell_tests) fn run_with_secret(
    fixture: &ShellFixture,
    secret: Zeroizing<String>,
    token_auth: bool,
    args: &[&str],
) -> ShellRun {
    fixture.guard.mark("shell: spawn");
    let (env_key, env_value) = profile_env(fixture);
    let mut argv: Vec<String> = vec![
        "shell".into(),
        "--profile".into(),
        PROFILE_NAME.into(),
        "--secret-stdin".into(),
    ];
    if token_auth {
        argv.extend(["--auth".into(), "token".into()]);
    }
    if args.contains(&"--") {
        // Caller-supplied shape: use exactly as given, no inserted or
        // duplicated delimiter.
        argv.extend(args.iter().map(ToString::to_string));
    } else if !args.is_empty() {
        argv.push("--".into());
        argv.extend(args.iter().map(ToString::to_string));
    }
    // An EMPTY `args` is a bare interactive probe: appending `--` there
    // would flip the parser into command-consuming mode with nothing after it.
    let needle = if token_auth {
        token_secret_suffix(&secret)
    } else {
        secret.as_str()
    };
    assert!(
        !argv.iter().any(|arg| {
            (!needle.is_empty() && arg.contains(needle)) || arg.contains(fixture.canary.as_str())
        }),
        "fixture secret leaked into the assembled argv",
    );
    // The child's whole home is the isolated profile home: no state can
    // land in the real user home even under a regression.
    let home_dir = profile_home_path(fixture);

    let outcome = fixture.rt.run(bound(SHELL_BOUND, async move {
        let mut command = server_command();
        command
            .args(&argv)
            .env(env_key, env_value)
            .current_dir(&home_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.env("HOME", &home_dir);
        #[cfg(windows)]
        command.env("USERPROFILE", &home_dir);
        let mut child = command.spawn().expect("spawn shell child");
        // stdin-only; close the pipe for EOF. An early-exit refusal closes
        // the pipe first, so only `BrokenPipe` is tolerated downstream.
        if let Some(mut stdin) = child.stdin.take() {
            write_secret_stdin(&mut stdin, secret.as_bytes()).await;
            drop(stdin);
        }
        let output = child
            .wait_with_output()
            .await
            .expect("shell child waits with output");
        (
            output.status.code().unwrap_or(-1),
            output.stdout,
            output.stderr,
        )
    }));
    fixture.guard.note("shell exit", outcome.0.to_string());
    ShellRun {
        exit_code: outcome.0,
        stdout: outcome.1,
        stderr: outcome.2,
    }
}

/// Write the canary line to the child's stdin, then flush+shutdown.
/// `ErrorKind::BrokenPipe` is accepted (early-exit refusal); every other
/// error is a fixture bug and panics. The line is `Zeroizing`-scrubbed.
async fn write_secret_stdin(stdin: &mut tokio::process::ChildStdin, secret: &[u8]) {
    // Preallocated (secret + LF), zeroized so the assembled line is
    // scrubbed on drop.
    let mut line = Zeroizing::new(Vec::with_capacity(secret.len() + 1));
    line.extend_from_slice(secret);
    line.push(b'\n');
    if let Err(error) = stdin.write_all(line.as_slice()).await {
        if error.kind() != std::io::ErrorKind::BrokenPipe {
            panic!("write secret to shell stdin: {error}");
        }
        return; // child exited before reading the secret — fine for refusals
    }
    if let Err(error) = stdin.flush().await {
        if error.kind() != std::io::ErrorKind::BrokenPipe {
            panic!("flush secret to shell stdin: {error}");
        }
        return;
    }
    if let Err(error) = stdin.shutdown().await {
        if error.kind() != std::io::ErrorKind::BrokenPipe {
            panic!("close shell stdin: {error}");
        }
    }
}
