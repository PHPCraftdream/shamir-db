//! The scripted REPL driven through a real child process (review items
//! 6a/6b): the entry test below runs the production
//! [`crate::shell::parser::run::run_scripted`] loop with the real process stdin
//! and stderr, so the parent captures INDEPENDENT real stdout and stderr
//! pipes — the human result stays on stderr by design; the child's stdout
//! legitimately carries only libtest framing, and the parent scans ALL
//! captured bytes for secrets instead of assuming silence. The secret
//! travels as the FIRST stdin line (dedicated pipe, never argv and never
//! in a test error message). This file also hosts the shared run-record
//! assertions and the shared post-run leak discipline helpers used by
//! every probe in this folder.

use std::io::{self, Read};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use zeroize::Zeroizing;

use tokio::io::AsyncWriteExt;

use crate::shell::output::ShellError;
use crate::shell::parser::run::run_scripted;
use crate::shell::terminal::secret::{HiddenLineSession, Terminal};

use crate::shell_tests::harness::{ShellRun, PROFILE_NAME};
use crate::shell_tests::secure_fixture::{
    scan_operator_home, scripted_child_command, secret_suffix,
};

/// Generous hang bound for one child run (Windows runners are up to 10x
/// slower; the idle probe stages its own longer bounds).
const CHILD_BOUND: Duration = Duration::from_secs(180);

/// Selects the child entry test in the spawned test binary.
pub(in crate::shell_tests) const CHILD_GATE: &str = "SHAMIR_SHELL_SCRIPTED_CHILD";

/// The exact libtest name of the child entry. Libtest omits the first
/// crate segment (`admin::` here), so `concat!(module_path!(), ...)` would
/// produce a name the `--exact` filter never matches; this is the known
/// exact name instead.
pub(in crate::shell_tests) const CHILD_ENTRY: &str =
    "shell_tests::secure_transports::capture::shell_scripted_repl_child_entry";

/// Secret-line cap, mirroring the shell's own bounded reader.
const SECRET_LINE_MAX_BYTES: usize = 4096;

/// Fixed messages and codes shared by the probes in this folder.
pub(in crate::shell_tests) const DENIED: &str = "permission denied by the server";
pub(in crate::shell_tests) const UNKNOWN_RESOURCE: &str =
    "the addressed resource does not exist on this server";
pub(in crate::shell_tests) const CONNECT_FAILURE: &str = "connection to the server failed";
pub(in crate::shell_tests) const TOKEN_SUBSET: &str =
    "token sessions expose only the read-only shell subset; log in with SCRAM for this command";

// ── Child entry ──────────────────────────────────────────────────────

/// The spawned child runs the REAL interactive dispatch against the real
/// process handles: human output on stderr (design §5), stdout untouched
/// by the shell. Without the gate (a normal `cargo test` invocation of
/// this binary) it does nothing.
#[test]
fn shell_scripted_repl_child_entry() {
    if std::env::var(CHILD_GATE).is_err() {
        return;
    }
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let secret = read_secret_line(&mut input);
    let terminal = PromptSecret(secret);
    let root = profile_root_from_env();
    let exit = run_scripted(
        ["-q".into(), "--profile".into(), PROFILE_NAME.into()],
        &mut input,
        &mut io::stderr(),
        &terminal,
        Some(root.as_path()),
    );
    // The shell exit is asserted HERE (never a secret in the message) and
    // a normal return lets the PromptSecret secret scrub its buffer; the
    // parent propagates the libtest status. This process's stdout carries
    // libtest framing, so the parent never asserts stdout emptiness — it
    // scans ALL bytes for secrets instead.
    assert_eq!(exit, 0, "scripted REPL exited {exit}");
}

/// Reads the first stdin line (the secret) with the shell's byte bound.
/// The value is never placed in a panic message.
fn read_secret_line(input: &mut dyn Read) -> Zeroizing<Vec<u8>> {
    let mut line = Zeroizing::new(Vec::with_capacity(64));
    let mut byte = [0u8; 1];
    loop {
        let read = input
            .read(&mut byte)
            .unwrap_or_else(|_| panic!("child stdin secret read failed"));
        assert!(read == 1, "child stdin closed before the secret line");
        if byte[0] == b'\n' {
            break;
        }
        assert!(
            line.len() < SECRET_LINE_MAX_BYTES,
            "child secret line exceeds the bound"
        );
        line.push(byte[0]);
    }
    assert!(!line.is_empty(), "child secret line must not be empty");
    line
}

/// The operator profile root, resolved from the same variables the live
/// dispatch reads (set by the fixture for the child only).
fn profile_root_from_env() -> PathBuf {
    let home = if cfg!(unix) {
        std::env::var("XDG_CONFIG_HOME")
    } else {
        std::env::var("LOCALAPPDATA")
    }
    .expect("profile home env in scripted child");
    let home = PathBuf::from(home);
    if cfg!(unix) {
        home.join("shamir-shell").join("profiles")
    } else {
        home.join("ShamirDB").join("shell").join("profiles")
    }
}

/// The existing scripted seam drives the production REPL without a
/// platform PTY: the secret arrives through the hidden-prompt seam.
struct PromptSecret(Zeroizing<Vec<u8>>);

impl Terminal for PromptSecret {
    fn begin_hidden_read(&self) -> Result<Box<dyn HiddenLineSession + '_>, ShellError> {
        Ok(Box::new(PromptSecret(self.0.clone())))
    }
}

impl HiddenLineSession for PromptSecret {
    fn read_line(&mut self) -> Result<Zeroizing<Vec<u8>>, ShellError> {
        Ok(std::mem::take(&mut self.0))
    }

    fn restore(self: Box<Self>) -> Result<(), ShellError> {
        Ok(())
    }
}

// ── Parent side ──────────────────────────────────────────────────────

/// One scripted-REPL run in the child: real captured stdout AND stderr,
/// both scanned for the secret and its suffix, operator home scanned
/// afterwards (6b). The first stdin line carries the secret; `script` is
/// the remaining command lines (end with `quit` or rely on EOF).
pub(in crate::shell_tests) fn scripted_child(
    fixture: &crate::shell_tests::harness::ShellFixture,
    secret: &Zeroizing<String>,
    script: &str,
) -> ShellRun {
    fixture.guard.mark("shell: scripted child");
    let mut payload = Vec::with_capacity(secret.len() + 1 + script.len());
    payload.extend_from_slice(secret.as_bytes());
    payload.push(b'\n');
    payload.extend_from_slice(script.as_bytes());
    let payload = Zeroizing::new(payload);
    let mut command = scripted_child_command(fixture, CHILD_GATE, CHILD_ENTRY);
    let outcome = fixture.rt.run(bound(CHILD_BOUND, async move {
        let mut child = command.spawn().expect("spawn scripted child");
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(payload.as_slice())
                .await
                .expect("write scripted child input");
            // Drop the owned stdin pipe: the script terminates on its own
            // LF or on EOF.
        }
        let output = child
            .wait_with_output()
            .await
            .expect("scripted child waits with output");
        (
            output.status.code().unwrap_or(-1),
            output.stdout,
            output.stderr,
        )
    }));
    fixture
        .guard
        .note("scripted child exit", outcome.0.to_string());
    let run = ShellRun {
        exit_code: outcome.0,
        stdout: outcome.1,
        stderr: outcome.2,
    };
    assert_clean(&run, secret.as_str());
    assert_eq!(run.exit_code, 0, "scripted child exited nonzero");
    // The secret itself is already asserted in `assert_clean`; the suffix
    // gets its own all-streams check via `contains_canary`.
    assert!(
        !run.contains_canary(secret_suffix(secret)),
        "secret suffix leaked onto a child stream"
    );
    let needles = [secret.as_str(), secret_suffix(secret)];
    scan_operator_home(fixture, &needles);
    run
}

// ── Shared run-record assertions ─────────────────────────────────────

pub(in crate::shell_tests) fn assert_clean(run: &ShellRun, secret: &str) {
    assert!(
        !run.contains_canary(secret),
        "secret leaked onto a child stream"
    );
    for stream in [&run.stdout, &run.stderr] {
        assert!(
            stream
                .iter()
                .all(|&byte| !matches!(byte, b'\t' | 0x07 | 0x1b)),
            "unexpected raw TAB, BEL or ESC on a child stream"
        );
    }
}

pub(in crate::shell_tests) fn record(run: &ShellRun, command: &str) -> Value {
    let line = run
        .stdout
        .strip_suffix(b"\n")
        .expect("LF-terminated stdout");
    assert!(!line.contains(&b'\n'), "expected a single stdout record");
    let value: Value = serde_json::from_slice(line).expect("valid JSON record");
    assert!(value["version"].as_u64() == Some(1), "expected v1");
    assert!(
        value["command"].as_str() == Some(command),
        "wrong command ID"
    );
    value
}

pub(in crate::shell_tests) fn result(run: &ShellRun, command: &str, op: Option<&str>) -> Value {
    assert_eq!(run.exit_code, 0, "{command}: expected success");
    let value = record(run, command);
    assert!(value["kind"].as_str() == Some("result"), "expected result");
    assert!(
        value.get("op").is_some_and(|value| match op {
            Some(name) => value.as_str() == Some(name),
            None => value.is_null(),
        }),
        "wrong op"
    );
    assert!(
        value.get("node").is_some_and(Value::is_null),
        "unexpected node"
    );
    assert!(value.get("code").is_none() && value.get("message").is_none());
    assert!(!value["data"].is_null(), "missing result data");
    value
}

pub(in crate::shell_tests) fn error(
    run: &ShellRun,
    command: &str,
    exit: i32,
    code: &str,
    message: &str,
) {
    assert_eq!(run.exit_code, exit, "{command}: expected {code}");
    let value = record(run, command);
    assert!(value["kind"].as_str() == Some("error"), "expected error");
    assert!(value["code"].as_str() == Some(code), "wrong error code");
    assert!(
        value["message"].as_str() == Some(message),
        "wrong fixed message"
    );
    assert!(value.get("data").is_some_and(Value::is_null));
    assert!(
        run.stderr.is_empty(),
        "unexpected diagnostics in default batch mode"
    );
}

pub(in crate::shell_tests) fn assert_member(items: &Value, member: &str) {
    let items = items.as_array().expect("expected catalogue array");
    assert!(
        items.iter().any(|item| item.as_str() == Some(member)),
        "missing fixture member"
    );
}

// ── Hang bound ───────────────────────────────────────────────────────

/// Absolute-bound wrapper (same discipline as the harness `bound`): an
/// elapse means a genuine hang, never a slow runner.
async fn bound<T>(limit: Duration, future: impl std::future::Future<Output = T>) -> T {
    match tokio::time::timeout(limit, future).await {
        Ok(value) => value,
        Err(_) => panic!("scripted child step exceeded its {limit:?} hang bound"),
    }
}
