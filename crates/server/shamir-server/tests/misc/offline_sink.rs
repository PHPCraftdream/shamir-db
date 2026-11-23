//! Behavioural test: an offline command refused by its data-directory lock
//! must not have created the diagnostic log sink its config asks for.
//!
//! Mutant killed: reverting the offline sink selection in
//! `crates/server/shamir-server/src/main.rs` (`sink_mode_for(offline)`, i.e.
//! `SinkMode::StdoutOnly` for every offline subcommand) to `SinkMode::Config`.
//! With `SinkMode::Config`, `logging::init_for` builds the batched file writer,
//! whose `OpenOptions::new().create(true).append(true).open(path)`
//! (`shamir-server-security/src/security/observability/logging/mod.rs:348`)
//! runs during boot — i.e. BEFORE the token CLI takes, and is refused, the
//! offline maintenance lock — so `server.log` exists even though the command
//! did nothing. With `SinkMode::StdoutOnly` the configured path is never
//! opened, created or stat-ed.
//!
//! Parent commit: `6ef25baaf` (the commit before the fix) called
//! `shamir_server::logging::init(&config.logging)` unconditionally in
//! `run_async`, so the sink was always created and the `!exists()` assertion
//! below fails there too. The parent's fix (`ef4ef5b5d`) already passes.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use shamir_server::config::Config;

/// Generous hang detector for the child process; not a latency assertion.
const WATCHDOG: Duration = Duration::from_secs(120);

/// Exact contention text of `LockFailure::Contention`
/// (`crates/server/shamir-server/src/boot/policy/token_cli/commands/lock.rs:18`).
/// Asserting it — rather than only a non-zero exit — keeps this test from
/// passing when the child failed for an unrelated reason (unparsable config,
/// bad arguments), which would also exit non-zero without creating a sink.
const LOCK_CONTENTION_TEXT: &str = "offline token command requires a stopped server";

/// Render a path for a Ktav scalar with forward slashes only, so a Windows
/// backslash can never be read as an escape by the Ktav parser.
fn ktav_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Minimal one-listener config, same shape as
/// `tests/seed_recovery_e2e/harness/child.rs::ktav_config`, plus the
/// `logging.file` diagnostic sink this test must observe absent.
fn config_text(data_dir: &Path, log_file: &Path) -> String {
    let data_dir = ktav_path(data_dir);
    format!(
        "\
data_dir: {data_dir}

logging: {{
    level: warn
    file: {log_file}
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
        addr: 127.0.0.1:0
        profile: tls_exporter
    }}
]

tls: {{
    cert_path: {data_dir}/cert.pem
    key_path: {data_dir}/key.pem
}}
",
        log_file = ktav_path(log_file),
    )
}

/// An offline `token create` refused by the data-directory lock must exit
/// non-zero and must not have created the configured diagnostic sink
/// (`logging.file`) anywhere in the data directory.
///
/// The lock is taken through `shamir_sync::durability::maintenance_lock::acquire`
/// — the exact function `shamir-server`'s own `maintenance_lock` module
/// re-exports for the CLI, wrapped by
/// `boot/policy/token_cli/commands/lock.rs:42`. (`shamir_server::maintenance_lock`
/// is `pub(crate)`, so it is not nameable from an integration test; importing
/// `shamir_sync` directly uses the same function, not a copy.)
///
/// Not covered here:
/// - the Windows-service entry (`windows_service::run`), which has no CLI
///   shape and is exercised by no process-level test here;
/// - files OTHER than the configured sink: the assertions say nothing about
///   whether the refused command created e.g. a directory or the lock file
///   itself (the lock file necessarily exists — this test creates it);
/// - a command refused for a non-lock reason after logging init.
#[test]
fn offline_command_refused_by_its_lock_creates_no_sink_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let data_dir = temp.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    let log_file = data_dir.join("server.log");
    let config_path = temp.path().join("shamir-server.ktav");
    std::fs::write(&config_path, config_text(&data_dir, &log_file)).expect("write config");

    // Precondition, checked in this process: the config parses and validates.
    // A config error would make the child exit non-zero BEFORE logging init,
    // i.e. without creating the sink, and would fake a pass — so the sink
    // assertion below is only meaningful with this proven.
    let parsed = Config::from_file(&config_path).expect("config parses");
    parsed.validate().expect("config validates");

    // The lock the offline CLI takes. Held (named `_guard`, not `_`) for the
    // whole test: dropping it would reopen the data directory.
    let _guard = shamir_sync::durability::maintenance_lock::acquire(&data_dir)
        .expect("take the offline maintenance lock");

    // Child streams go to files (not pipes), so a chatty child can never block
    // on a full pipe before exiting.
    let stdout_path = temp.path().join("child.stdout");
    let stderr_path = temp.path().join("child.stderr");
    let out_file = temp.path().join("token.out");
    let mut child = Command::new(env!("CARGO_BIN_EXE_shamir-server"))
        .arg("--config")
        .arg(&config_path)
        .arg("token")
        .arg("create")
        .arg("--user")
        .arg("admin")
        .arg("--label")
        .arg("x")
        .arg("--out-file")
        .arg(&out_file)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            std::fs::File::create(&stdout_path).expect("create child stdout file"),
        ))
        .stderr(Stdio::from(
            std::fs::File::create(&stderr_path).expect("create child stderr file"),
        ))
        .spawn()
        .expect("spawn shamir-server");

    // Bounded wait: a deadline over `try_wait`, not a sleep-based handshake.
    // The command's own lock refusal is what terminates it; the poll interval
    // only paces the deadline check.
    let deadline = Instant::now() + WATCHDOG;
    let status = loop {
        match child.try_wait().expect("poll the child process") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("shamir-server exceeded its {WATCHDOG:?} hang bound");
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let mut stderr_text = String::new();
    std::fs::File::open(&stderr_path)
        .expect("reopen child stderr file")
        .read_to_string(&mut stderr_text)
        .expect("read child stderr");

    assert!(
        !status.success(),
        "a command refused by the offline maintenance lock must exit non-zero \
         (stderr: {stderr_text})"
    );
    assert!(
        stderr_text.contains(LOCK_CONTENTION_TEXT),
        "the child must have been refused BY ITS LOCK, not for another reason; \
         stderr was: {stderr_text}"
    );
    assert!(
        !log_file.exists(),
        "an offline command must not create the configured diagnostic sink \
         before (or after) its lock refusal, but {} exists",
        log_file.display()
    );
}
