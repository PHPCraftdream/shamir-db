//! Real leader CHILD process + real CLI subcommand runners.
//!
//! The leader is the actual `shamir-server` binary launched with a
//! generated `.ktav` config. "Crash" means `Child::start_kill()` (SIGKILL on
//! Unix): no graceful shutdown, no flush-on-exit hooks. This certifies
//! PROCESS-crash behaviour only (see the module docs in `main.rs`).
//!
//! Seed export / verify / install are driven through the real CLI
//! subcommands of the same binary, exactly as an operator would run them.

use std::net::SocketAddr;
use std::net::TcpListener;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Child;

use shamir_client::Client;
use shamir_client::ConnectOptions;
use zeroize::Zeroizing;

/// The binary under test — resolved by cargo for integration targets.
pub const SERVER_BIN: &str = env!("CARGO_BIN_EXE_shamir-server");

pub const ADMIN: &str = "admin";
pub const ADMIN_PW: &str = "admin-password-e2e";
pub const REPLICATOR: &str = "replicator";
pub const REPLICATOR_PW: &str = "repl-secret-e2e";

const BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// A running leader child process on a fixed loopback port.
pub struct LeaderChild {
    pub child: Child,
    pub addr: SocketAddr,
    pub config_path: PathBuf,
    /// Combined stdout/stderr of the child, surfaced when boot fails.
    pub log_path: PathBuf,
}

/// Write a `.ktav` config for CLI subcommands operating on `data_dir`
/// (the export/install tools take `--config` and read its `data_dir`).
pub fn write_home_config(data_dir: &Path, addr: SocketAddr) -> PathBuf {
    std::fs::create_dir_all(data_dir).expect("create home data dir");
    let config_path = data_dir.join("follower.ktav");
    std::fs::write(&config_path, ktav_config(data_dir, addr)).expect("write home config");
    config_path
}

/// Claim a free loopback port race-free (bind port 0, read it, drop).
pub fn free_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind :0");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    addr
}

/// Spec-floor-but-fast KDF, matching `tests/common/mod.rs::fast_kdf`.
fn ktav_config(data_dir: &Path, addr: SocketAddr) -> String {
    format!(
        "\
data_dir: {data_dir}

logging: {{
    level: warn
}}

kdf_defaults: {{
    memory_kb: 19456
    time: 2
    parallelism: 1
    argon2_version: 19
}}

argon2_concurrent_max: 4

listeners: [
    {{
        kind: tcp
        addr: {addr}
        profile: tls_exporter
    }}
]

tls: {{
    cert_path: {data_dir}/cert.pem
    key_path: {data_dir}/key.pem
}}
",
        data_dir = data_dir.display(),
        addr = addr,
    )
}

/// Write the config file and launch the leader child. Does NOT wait for
/// readiness — call [`connect_admin`], which polls until the child serves.
pub async fn spawn_leader(data_dir: &Path, addr: SocketAddr) -> LeaderChild {
    std::fs::create_dir_all(data_dir).expect("create leader data dir");
    let config_path = data_dir.join("leader.ktav");
    std::fs::write(&config_path, ktav_config(data_dir, addr)).expect("write leader config");

    let log_path = data_dir.join("leader.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .expect("open leader log");
    let log_err = log.try_clone().expect("clone leader log handle");
    let child = tokio::process::Command::new(SERVER_BIN)
        .arg("--config")
        .arg(&config_path)
        .arg("--bootstrap-user")
        .arg(ADMIN)
        .arg("--bootstrap-password")
        .arg(ADMIN_PW)
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .expect("spawn shamir-server child");
    LeaderChild {
        child,
        addr,
        config_path,
        log_path,
    }
}

/// Last bytes of the child's log, for failure messages.
fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| format!("<unreadable: {e}>"));
    let keep = text.len().saturating_sub(4000);
    let start = (keep..text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(text.len());
    text[start..].to_owned()
}

/// Connect an admin TLS+SCRAM client, retrying until the child is serving
/// (bounded — no fixed startup sleep). Every returned client is a live
/// session the caller drives over the real wire.
pub async fn connect_admin(leader: &LeaderChild) -> Client {
    wait_listening(leader.addr, &leader.log_path).await;
    let deadline = tokio::time::Instant::now() + BOOT_DEADLINE;
    loop {
        match Client::connect(admin_options(leader.addr, ADMIN, ADMIN_PW)).await {
            Ok(client) => return client,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "leader admin login did not succeed in time: {error}
leader log tail:
{}",
                    log_tail(&leader.log_path),
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

/// Poll a plain TCP connect until the child's listener accepts (or panic at
/// the boot deadline). The listener only exists once boot reached the
/// bind-listeners stage, so this is an observable readiness signal.
async fn wait_listening(addr: SocketAddr, log_path: &Path) {
    let deadline = tokio::time::Instant::now() + BOOT_DEADLINE;
    loop {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "leader child never started listening on {addr}
leader log tail:
{}",
            log_tail(log_path),
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// TLS + SCRAM login as `user` (spec-floor-fast KDF comes from the config).
pub async fn connect(addr: SocketAddr, user: &str, password: &str) -> Client {
    Client::connect(admin_options(addr, user, password))
        .await
        .expect("client connect")
}

fn admin_options(addr: SocketAddr, user: &str, password: &str) -> ConnectOptions {
    ConnectOptions {
        addr,
        server_name: "localhost".to_string(),
        username: user.to_string(),
        password: Zeroizing::new(password.as_bytes().to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: Some(Duration::from_secs(15)),
        request_timeout: Some(Duration::from_secs(35)),
    }
}

/// SIGKILL the leader and reap it. Advisory file locks (maintenance,
/// fjall single-instance) die with the process, so the data directory is
/// immediately available to the CLI tools afterwards.
pub async fn kill(leader: &mut LeaderChild) {
    leader.child.start_kill().expect("start_kill");
    leader.child.wait().await.expect("reap killed leader");
}

/// Run a one-shot CLI subcommand of the real binary. Panics with captured
/// stderr on a non-zero exit unless `expect_failure`.
pub async fn run_cli(config_path: &Path, args: &[&str]) -> (bool, String) {
    let output = tokio::process::Command::new(SERVER_BIN)
        .arg("--config")
        .arg(config_path)
        .args(args)
        .output()
        .await
        .expect("run CLI subcommand");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

pub async fn expect_cli(config_path: &Path, args: &[&str]) -> String {
    let (ok, text) = run_cli(config_path, args).await;
    assert!(ok, "CLI {args:?} failed:\n{text}");
    text
}

/// `replication-seed-export --db app --repo main --out <dir>`; returns the
/// trusted manifest SHA-256 the CLI printed.
pub async fn cli_export(config_path: &Path, out: &Path) -> String {
    let text = expect_cli(
        config_path,
        &[
            "replication-seed-export",
            "--db",
            "app",
            "--repo",
            "main",
            "--out",
            out.to_str().expect("utf8 out path"),
        ],
    )
    .await;
    let prefix = "manifest sha256: ";
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(str::trim)
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("export printed no digest: {text}"))
}

/// `replication-seed-verify <dir> --manifest-digest <hex>`.
pub async fn cli_verify(config_path: &Path, artifact: &Path, digest: &str) {
    expect_cli(
        config_path,
        &[
            "replication-seed-verify",
            artifact.to_str().expect("utf8 artifact path"),
            "--manifest-digest",
            digest,
        ],
    )
    .await;
}

/// `replication-seed-install` creating the paused receipt-bound
/// subscription; `expected_success = false` asserts a non-zero exit and
/// returns the captured output instead of panicking.
pub async fn cli_install(
    config_path: &Path,
    artifact: &Path,
    digest: &str,
    upstream: &str,
) -> String {
    expect_cli(
        config_path,
        &[
            "replication-seed-install",
            "--db",
            "app",
            "--repo",
            "main",
            "--artifact",
            artifact.to_str().expect("utf8 artifact path"),
            "--manifest-digest",
            digest,
            "--subscription",
            "sub",
            "--upstream",
            upstream,
            "--publication",
            "pub",
            "--profile",
            "seedprof",
        ],
    )
    .await
}

/// Like [`cli_install`] but asserting the install CLI REFUSES (non-zero
/// exit) and returning its output for code assertions.
pub async fn cli_install_refused(config_path: &Path, artifact: &Path, digest: &str) -> String {
    let (ok, text) = run_cli(
        config_path,
        &[
            "replication-seed-install",
            "--db",
            "app",
            "--repo",
            "main",
            "--artifact",
            artifact.to_str().expect("utf8 artifact path"),
            "--manifest-digest",
            digest,
            "--subscription",
            "sub",
            "--upstream",
            "tcp://127.0.0.1:1",
            "--publication",
            "pub",
            "--profile",
            "seedprof",
        ],
    )
    .await;
    assert!(!ok, "install should refuse, but succeeded:\n{text}");
    text
}
