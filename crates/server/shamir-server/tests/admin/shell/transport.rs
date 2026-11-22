//! PR1 runtime acceptance for the shell's transports (brief
//! `54-r5-1-shell-foundation.md`, design `admin-shell.md` §3.2/§5/§8): the
//! real production binary through the shared [`super::harness`] fixture (one
//! real server: TLS-TCP + IPC, verified pin, unique canary, catalogue seeded
//! over the wire). One test per topic: tcp/ipc ping + catalogue matrices;
//! every §3.2 typed refusal plus auth/resume routing; `--secret-fd`/env;
//! insecure profile leaves with restore controls; catalogue DATA reflection
//! (escaped text only, byte-scanned, decoded equality); real non-superuser
//! denial (exit 4) with auth controls; scripted interactive REPL behind a
//! fake terminal; hygiene. No sleeps; every await bounded by the harness
//! 120 s hang bounds and the `FlakeGuard::with_watchdog` dump.

use serde_json::Value;
use std::io::Cursor;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use tokio::io::AsyncWriteExt as _;

use zeroize::Zeroizing;

use crate::shell::output::ShellError;
use crate::shell::parser::run::run_scripted;
use crate::shell::terminal::secret::HiddenLineSession;
use crate::shell::terminal::secret::Terminal;

use super::harness::{
    boot, overwrite_profile, profile_env, profile_text, run_shell, run_shell_ok, seed_catalogue,
    seed_shell_user, write_profile, write_profile_with_pin, ShellFixture, ShellRun, DB_NAME,
    FN_NAME, INDEXED_TABLE, INDEX_NAME, INDEX_ODD_NAME, PLAIN_TABLE, PROFILE_NAME, REPO_NAME,
    SCHEMA_TABLE, SHELL_USER,
};

/// `shell/output.rs` `sanitize` (upper hex `{:04X}`) of [`super::harness::INDEX_ODD_NAME`]: the DECODED record string the projection emits — backslash-u literals, the exact expected text.
const INDEX_ODD_ESCAPED: &str = "odd\\u0009index\\u001B[31m\\u0007";

/// The INTERACTIVE human rendering of that name: JSON re-render doubles each literal backslash, THEN sanitize turns each `\` into `\u005C`.
const INDEX_ODD_HUMAN_FRAGMENT: &str = "odd\\u005C\\u005Cu0009";
const INDEX_ODD_HUMAN_FULL: &str =
    "odd\\u005C\\u005Cu0009index\\u005C\\u005Cu001B[31m\\u005C\\u005Cu0007";

// ── Envelope helpers (design §5 field table) ─────────────────────────

/// Parse the child's stdout as EXACTLY one LF-terminated JSON Lines v1 record.
fn parse_one_record(run: &ShellRun) -> Value {
    assert!(run.stdout.last() == Some(&b'\n'), "no LF terminator");
    let lines: Vec<&[u8]> = run.stdout.split_inclusive(|byte| *byte == b'\n').collect();
    assert_eq!(lines.len(), 1, "expected one record, got {}", lines.len());
    let record: Value = serde_json::from_slice(lines[0])
        .unwrap_or_else(|error| panic!("stdout record is not valid JSON: {error}"));
    assert_eq!(record["version"].as_u64(), Some(1), "not v1");
    record
}

/// Assert a successful result record: kind/op/command/node contract.
fn expect_result(record: &Value, command: &str, op: Option<&str>) {
    assert_eq!(record.get("command"), Some(&Value::String(command.into())));
    assert_eq!(record.get("kind"), Some(&Value::String("result".into())));
    match op {
        Some(op) => assert_eq!(record.get("op"), Some(&Value::String(op.into()))),
        None => assert!(record.get("op").is_some_and(Value::is_null)),
    }
    // `node` is always null in PR1 (never a fabricated observation).
    assert!(record["node"].is_null(), "node is null");
    assert!(record.get("data").is_some_and(|d| !d.is_null()), "has data");
    assert!(record.get("code").is_none(), "result carries no code");
    assert!(record.get("message").is_none(), "result has no message");
}

/// Assert an error record: kind/code/data-null contract, exit code, and the exact fixed local message when `message` is `Some`.
fn expect_error(
    run: &ShellRun,
    record: &Value,
    command: &str,
    code: &str,
    exit: i32,
    message: Option<&str>,
) {
    assert_eq!(run.exit_code, exit, "exit mismatch for {command} ({code})");
    assert_eq!(record.get("command"), Some(&Value::String(command.into())));
    assert_eq!(record.get("kind"), Some(&Value::String("error".into())));
    assert_eq!(record.get("code"), Some(&Value::String(code.into())));
    assert!(record["message"].is_string(), "has message");
    assert!(record["data"].is_null(), "data is null");
    if let Some(expected) = message {
        assert_eq!(
            record.get("message").and_then(Value::as_str),
            Some(expected),
            "fixed local message for {command}"
        );
    }
}

fn assert_no_leak(fixture: &ShellFixture, run: &ShellRun) {
    assert!(!run.contains_canary(fixture.canary.as_str()), "canary leak");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!stderr.contains("panicked at"), "panic echoed");
}

/// `run_shell_ok` + one `ping` result record (the restore-control probe).
fn ping_ok(fixture: &ShellFixture) {
    let run = run_shell_ok(fixture, &["ping"]);
    expect_result(&parse_one_record(&run), "ping", Some("Ping"));
}

/// Exactly ONE file in the profile home — the profile itself (no history file, design §5).
fn assert_no_created_files(fixture: &ShellFixture) {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    walk(fixture.profile_home.path(), &mut files, &mut dirs);
    assert_eq!(files.len(), 1, "profile home holds {} files", files.len());
    let name = files[0].file_name().and_then(std::ffi::OsStr::to_str);
    assert_eq!(name, Some("fixture.ktav"), "only the profile leaf");
    // Exactly the config root → shell → profiles chain on both platforms.
    for dir in &dirs {
        let depth = dir
            .strip_prefix(fixture.profile_home.path())
            .expect("inside home");
        assert!(depth.components().count() <= 3, "unexpected directory");
    }
}

/// Pull a JSON array-of-strings field out of a result record.
fn str_field<'a>(record: &'a Value, pointer: &str, expect: &str) -> Vec<&'a str> {
    record
        .pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("missing {expect}"))
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

fn walk(
    dir: &std::path::Path,
    files: &mut Vec<std::path::PathBuf>,
    dirs: &mut Vec<std::path::PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path.clone());
            walk(&path, files, dirs);
        } else {
            files.push(path);
        }
    }
}

/// The canary never appears in any byte of the profile file (design §3.2).
fn assert_profile_clean(fixture: &ShellFixture, path: &std::path::Path) {
    let text = std::fs::read_to_string(path).expect("profile readable");
    assert!(!text.contains(fixture.canary.as_str()));
}

// ── Catalogue matrix (shared by both transports) ─────────────────────

/// Ping + the whole catalogue/describe/schema set under the written profile: full v1 contract + seeded data.
fn run_catalogue_matrix(fixture: &ShellFixture) {
    /// One matrix step: mark, run, leak-scan, parse, assert the contract.
    fn step(fixture: &ShellFixture, args: &[&str], command: &str, op: Option<&str>) -> Value {
        fixture.guard.mark("matrix");
        let run = run_shell_ok(fixture, args);
        assert_no_leak(fixture, &run);
        let record = parse_one_record(&run);
        expect_result(&record, command, op);
        record
    }

    // Explicit successful connect: SCRAM + verified pin, `connected` flag.
    let record = step(fixture, &["connect"], "connect", None);
    assert_eq!(record.pointer("/data/connected"), Some(&Value::Bool(true)));

    // `ping`: non-batch top-level op, no routing-DB floor.
    step(fixture, &["ping"], "ping", Some("Ping"));
    assert_no_created_files(fixture);

    // `db list` → the seeded database must be listed.
    let record = step(fixture, &["db", "list"], "db list", Some("List"));
    assert!(str_field(&record, "/data/databases", "databases array").contains(&DB_NAME));

    // `repo list` → the seeded repo.
    let record = step(fixture, &["repo", "list"], "repo list", Some("List"));
    assert!(str_field(&record, "/data/repos", "repos array").contains(&REPO_NAME));

    // `table list REPO` → all three fixture tables listed.
    let record = step(
        fixture,
        &["table", "list", REPO_NAME],
        "table list",
        Some("List"),
    );
    let names = str_field(&record, "/data/tables", "table list tables array");
    assert!(
        names.contains(&PLAIN_TABLE)
            && names.contains(&INDEXED_TABLE)
            && names.contains(&SCHEMA_TABLE),
        "table list must contain the three fixture tables"
    );

    // `index list REPO TABLE` → the seeded index present.
    let args = &["index", "list", REPO_NAME, INDEXED_TABLE];
    let record = step(fixture, args, "index list", Some("List"));
    let found = record
        .pointer("/data/indexes")
        .and_then(Value::as_array)
        .expect("indexes array")
        .iter()
        .any(|entry| entry.get("name").and_then(Value::as_str) == Some(INDEX_NAME));
    assert!(found, "index list must contain the fixture index");

    // `function list` → the seeded function, exactly its name+kind projection.
    let record = step(
        fixture,
        &["function", "list"],
        "function list",
        Some("List"),
    );
    let functions = record
        .pointer("/data/functions")
        .and_then(Value::as_array)
        .expect("functions array");
    let seeded = functions
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(FN_NAME))
        .expect("function list must contain the seeded function");
    // WASM artifact; the server's `ArtifactKind::Wasm.as_str()` is "wasm".
    assert_eq!(seeded.get("kind").and_then(Value::as_str), Some("wasm"));
    for entry in functions {
        let entry = entry.as_object().expect("function entries are objects");
        assert_eq!(entry.len(), 2, "function entries project exactly name+kind");
        assert!(entry.get("kind").is_some_and(Value::is_string), "kind str");
    }

    // `table describe REPO TABLE` → the documented field table.
    let args = &["table", "describe", REPO_NAME, PLAIN_TABLE];
    let record = step(fixture, args, "table describe", Some("DescribeTable"));
    let data = record
        .get("data")
        .and_then(Value::as_object)
        .expect("object data");
    for field in ["schema", "owner", "group", "mode", "retention", "buffer"] {
        assert!(data.contains_key(field), "describe missing field {field}");
    }
    // Principal ids render as decimal strings (or stay null when unknown).
    for field in ["owner", "group"] {
        let entry = data.get(field).expect("principal field");
        assert!(
            entry.is_null() || entry.is_string(),
            "{field} must be decimal string or null"
        );
    }

    // `table schema REPO SCHEMA_TABLE` → seeded ACTUAL values: projected
    // names, the exact `Int`-as-string schema_version, BOTH seeded fields.
    let args = &["table", "schema", REPO_NAME, SCHEMA_TABLE];
    let record = step(fixture, args, "table schema", Some("GetTableSchema"));
    let got = record
        .pointer("/data/get_table_schema")
        .and_then(Value::as_str);
    assert_eq!(got, Some(SCHEMA_TABLE));
    let repo = record.pointer("/data/repo").and_then(Value::as_str);
    assert_eq!(repo, Some(REPO_NAME));
    // `Int` projects to decimal text (read via `as_str` + parse). The harness
    // applies the schema through ONE `SetTableSchema` call on a table created
    // WITHOUT a `schema_version` field: the server defaults the absent field
    // to 0 (`admin_schema.rs:401-405`) and then increments to 1
    // (`admin_schema.rs:498`), persisting and echoing 1
    // (`admin_schema.rs:579-584`); `GetTableSchema` reads that same catalogue
    // field back (`admin_schema.rs:968-983`) — so the projected version is 1.
    let version = record
        .pointer("/data/schema_version")
        .and_then(Value::as_str)
        .and_then(|text| text.parse::<u64>().ok());
    assert_eq!(version, Some(1), "schema_version is the projected Int 1");
    let schema = record
        .pointer("/data/schema")
        .and_then(Value::as_array)
        .expect("schema payload");
    assert!(!schema.is_empty(), "the schema payload must not be empty");
    // Flat wire form: every entry is an object with a `path` array and a `type` string.
    for entry in schema {
        assert!(entry.get("path").is_some_and(Value::is_array), "path array");
        assert!(entry.get("type").is_some_and(Value::is_string), "type str");
    }
    // Seeded ACTUAL rules decoded field-by-field (score → int + required, label → string).
    let rule = |path: &str| {
        schema
            .iter()
            .find(|entry| {
                entry.get("path").and_then(Value::as_array) == serde_json::json!([path]).as_array()
            })
            .unwrap_or_else(|| panic!("missing seeded schema rule {path}"))
    };
    assert_eq!(rule("score")["type"].as_str(), Some("int"));
    assert_eq!(rule("score")["required"].as_bool(), Some(true));
    assert_eq!(rule("label")["type"].as_str(), Some("string"));
    // `table describe` of the SAME schema table: `schema` is the NONEMPTY
    // rule list for this table.
    let args = &["table", "describe", REPO_NAME, SCHEMA_TABLE];
    let record = step(fixture, args, "table describe", Some("DescribeTable"));
    let described = record
        .pointer("/data/schema")
        .and_then(Value::as_array)
        .expect("schema array");
    assert!(
        !described.is_empty(),
        "schema table describe must be nonempty"
    );
    let score_json = serde_json::json!(["score"]);
    let score_path = score_json.as_array();
    assert!(
        described
            .iter()
            .any(|entry| entry.get("path").and_then(Value::as_array) == score_path),
        "the described schema contains the seeded score rule"
    );
}

/// The TLS-TCP transport profile (`write_profile`): ping + the full catalogue matrix over TLS+SCRAM.
#[test]
fn shell_transport_tcp_ping_and_catalogue() {
    let fixture = boot("shell_transport_tcp_ping_and_catalogue");
    let path = write_profile(&fixture);
    assert_profile_clean(&fixture, &path);
    seed_catalogue(&fixture);
    run_catalogue_matrix(&fixture);
}

/// The IPC transport profile: the same matrix over the local IPC endpoint (SCRAM + pin, no TLS — §3.2 T4).
#[test]
fn shell_transport_ipc_ping_and_catalogue() {
    let fixture = boot("shell_transport_ipc_ping_and_catalogue");
    // IPC profile: raw endpoint address, no SNI field (rejected on IPC).
    let text = pinned_profile(&fixture, "ipc", &fixture.ipc_addr, None, DB_NAME);
    let path = overwrite_profile(&fixture, &text);
    assert_profile_clean(&fixture, &path);
    seed_catalogue(&fixture);
    run_catalogue_matrix(&fixture);
}

// ── Typed refusal probes (§3.2) ──────────────────────────────────────

/// One refusal probe against a live fixture: typed code/exit + hygiene.
fn expect_refusal(fixture: &ShellFixture, args: &[&str], command: &str, code: &str, exit: i32) {
    let run = run_shell(fixture, args);
    assert_no_leak(fixture, &run);
    assert!(!String::from_utf8_lossy(&run.stderr).contains("panicked at"));
    let record = parse_one_record(&run);
    expect_error(&run, &record, command, code, exit, None);
}

/// Overwrite the profile with `text` and assert the `ping` refusal.
fn refuse_profile_text(fixture: &ShellFixture, text: String, code: &str, exit: i32) {
    overwrite_profile(fixture, &text);
    expect_refusal(fixture, &["ping"], "ping", code, exit);
}

/// `profile_text` with the fixture's verified pin substituted.
fn pinned_profile(
    fixture: &ShellFixture,
    transport: &str,
    address: &str,
    sni: Option<&str>,
    database: &str,
) -> String {
    profile_text(transport, address, sni, Some(database)).replacen(
        "pin: <PIN>\n",
        &format!("pin: {}\n", fixture.pin_hex()),
        1,
    )
}

/// The §3.2 typed refusals for `--auth token` (fixed local message).
const TOKEN_REFUSAL_MESSAGE: &str =
    "API tokens require HTTPS REST or the native token WSS listener";

/// The §3.2 typed deferral for browser/http-login auth (fixed local message).
const DEFERRAL_MESSAGE: &str =
    "browser profile and HTTP password login are deferred; HTTP login mints a reusable API token, not a session";

/// The typed resume refusal (fixed local message).
const RESUME_MESSAGE: &str = "session resume is never used by the admin shell; log in fresh";

/// The fixed Windows `--secret-fd` refusal message.
#[cfg(windows)]
const FD_MESSAGE: &str = "--secret-fd is Unix-only; use --secret-stdin";

/// The fixed non-superuser denial message.
const DENIAL_MESSAGE: &str = "permission denied by the server";

/// Every §3.2 refusal row PR1 supports: one probe per row, each a fresh
/// deliberate profile (or argv shape), each exit code the design §5 table.
#[test]
fn shell_transport_typed_refusals() {
    let fixture = boot("shell_transport_typed_refusals");
    // The good profile is written first; every probe below overwrites it.
    write_profile(&fixture);
    let pinned = |transport: &str, database: &str| {
        pinned_profile(
            &fixture,
            transport,
            &fixture.tls_addr.to_string(),
            Some("localhost"),
            database,
        )
    };

    // Missing pin (empty pin string omits the field): refusal before any dial.
    write_profile_with_pin(&fixture, "");
    expect_refusal(&fixture, &["ping"], "ping", "missing_pin", 6);

    // Wrong pin (well-formed 64 hex, not the server identity): exit 6.
    write_profile_with_pin(&fixture, &"ab".repeat(32));
    expect_refusal(&fixture, &["ping"], "ping", "pin_mismatch", 6);

    // Malformed pin (not 64 hex digits): strict profile parse failure.
    write_profile_with_pin(&fixture, "not-a-pin");
    expect_refusal(&fixture, &["ping"], "ping", "invalid_profile", 6);

    // Plaintext alias (loopback included): refused before any socket.
    refuse_profile_text(&fixture, pinned("plain", DB_NAME), "plaintext_refused", 6);

    // Token-only spellings still refuse the default SCRAM auth; native WSS
    // is now supported and its pin checks live in the PR2 refusal tests.
    for (transport, code) in [
        ("token", "unsupported"),
        ("https", "unsupported"),
        ("rest", "unsupported"),
        ("browser-wss", "deferred"),
        ("tls-resume", "resume_refused"),
    ] {
        refuse_profile_text(&fixture, pinned(transport, DB_NAME), code, 6);
    }

    // Token auth via `--auth token` is refused after the command parse —
    // one batch error record, the fixed local message, exit 6, no dial —
    // against a valid IPC profile AND (identically) the TLS-TCP profile.
    for (transport, address, sni) in [
        ("ipc", fixture.ipc_addr.clone(), None),
        ("tls-tcp", fixture.tls_addr.to_string(), Some("localhost")),
    ] {
        let profile = pinned_profile(&fixture, transport, &address, sni, DB_NAME);
        overwrite_profile(&fixture, &profile);
        let run = run_shell(&fixture, &["--auth", "token", "--", "ping"]);
        assert_no_leak(&fixture, &run);
        let record = parse_one_record(&run);
        expect_error(
            &run,
            &record,
            "ping",
            "token_transport_refused",
            6,
            Some(TOKEN_REFUSAL_MESSAGE),
        );
    }

    // Wrong credentials: a profile whose username does not exist — the
    // live server rejects SCRAM → exit 3.
    let ghost = pinned("tls-tcp", DB_NAME).replacen("username: admin\n", "username: ghost\n", 1);
    refuse_profile_text(&fixture, ghost, "authentication_failed", 3);

    // `--accept-new-host` does not exist in any stage: a malformed startup
    // option fails the clap parse — stderr-only refusal (fixed
    // `error: usage …` line, exit 1), no stdout record.
    let run = run_shell(&fixture, &["--accept-new-host", "--", "ping"]);
    assert_no_leak(&fixture, &run);
    // stderr-only fixed usage refusal, exit 1, no stdout, no argv echo.
    assert!(run.stdout.is_empty());
    assert_eq!(run.exit_code, 1);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("error: usage invalid command usage"));
    assert!(!stderr.contains("--accept-new-host\""));

    // Batch routing floor: the routing database named by the profile does
    // not exist — batch catalogue commands fail server-side (exit 5) while
    // the non-batch `ping` (no floor) still succeeds.
    let text = pinned("tls-tcp", "nosuchroute");
    overwrite_profile(&fixture, &text);
    seed_catalogue(&fixture);
    expect_refusal(&fixture, &["db", "list"], "db list", "server_error", 5);
    ping_ok(&fixture);
}

// ── `connect --auth` routing and `--resume` (design §3.2/§4.1) ───────

/// Production auth-kind routing as `parser.rs` dispatches it: browser/http-login defer; resume routes are the typed refusal (exit 6) BEFORE any dial (server never contacted).
#[test]
fn shell_transport_connect_auth_and_resume_routing() {
    let fixture = boot("shell_transport_connect_auth_and_resume_routing");
    // The good profile is written first (valid, but never dialed here).
    write_profile(&fixture);

    // `connect --auth browser` / `--auth http-login`: the typed deferral.
    for auth in ["browser", "http-login"] {
        let run = run_shell(&fixture, &["connect", "--auth", auth]);
        assert_no_leak(&fixture, &run);
        let record = parse_one_record(&run);
        expect_error(
            &run,
            &record,
            "connect",
            "deferred",
            6,
            Some(DEFERRAL_MESSAGE),
        );
    }

    // `connect --auth resume` (connect command shape) and top-level
    // `--resume -- ping` (after the batch command parse): the typed resume
    // refusal, exit 6, the same fixed message.
    for (args, command) in [
        (&["connect", "--auth", "resume"] as &[&str], "connect"),
        (&["--resume", "--", "ping"] as &[&str], "ping"),
    ] {
        let run = run_shell(&fixture, args);
        assert_no_leak(&fixture, &run);
        let record = parse_one_record(&run);
        expect_error(
            &run,
            &record,
            command,
            "resume_refused",
            6,
            Some(RESUME_MESSAGE),
        );
    }
}

// ── Direct secret-source probes (the harness always prepends
// `--secret-stdin`, so secret-source tests spawn the REAL binary with the
// exact operator argv through one shared bounded helper) ──────────────

/// stdin payload of a direct spawn: null, or piped bytes then EOF.
enum DirectStdin {
    /// stdin null — the source is refused or read elsewhere (fd N, env).
    Null,
    /// The bytes are piped on fd 0, then the pipe is closed (EOF).
    Bytes(Zeroizing<Vec<u8>>),
}

/// One bounded direct spawn of the real binary: isolated profile home, CHILD-only `extra_env`, canary never on argv.
fn spawn_direct(
    fixture: &ShellFixture,
    argv: Vec<&'static str>,
    stdin: DirectStdin,
    extra_env: Vec<(&'static str, String)>,
) -> ShellRun {
    assert!(
        !argv.iter().any(|arg| arg.contains(fixture.canary.as_str())),
        "fixture canary leaked into the assembled argv",
    );
    let (env_key, env_value) = profile_env(fixture);
    let home = fixture.profile_home.path().to_path_buf();
    fixture.guard.mark("shell: direct spawn");
    let outcome = fixture.rt.run(async move {
        tokio::time::timeout(Duration::from_secs(120), async move {
            let mut command = super::harness::server_command();
            command
                .args(&argv)
                .env(env_key, env_value)
                .current_dir(&home)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            let command = command.env("HOME", &home);
            #[cfg(windows)]
            let command = command.env("USERPROFILE", &home);
            for (key, value) in extra_env {
                command.env(key, value);
            }
            let output = match stdin {
                DirectStdin::Null => command
                    .stdin(std::process::Stdio::null())
                    .output()
                    .await
                    .expect("spawn shell child"),
                DirectStdin::Bytes(bytes) => {
                    command.stdin(std::process::Stdio::piped());
                    let mut child = command.spawn().expect("spawn shell child");
                    let mut pipe = child.stdin.take().expect("piped stdin");
                    pipe.write_all(&bytes)
                        .await
                        .expect("write the secret payload");
                    drop(pipe);
                    child.wait_with_output().await.expect("shell child")
                }
            };
            (
                output.status.code().unwrap_or(-1),
                output.stdout,
                output.stderr,
            )
        })
        .await
        .expect("direct spawn bounded by the 120 s hang bound")
    });
    fixture.guard.note("shell exit", outcome.0.to_string());
    ShellRun {
        exit_code: outcome.0,
        stdout: outcome.1,
        stderr: outcome.2,
    }
}

/// Operator argv prefix for a direct spawn (harness `run_shell` shape minus its always-on `--secret-stdin`).
fn shell_argv(extra: &[&'static str]) -> Vec<&'static str> {
    let mut argv: Vec<&'static str> = vec!["shell", "--profile", PROFILE_NAME];
    argv.extend_from_slice(extra);
    argv
}

/// Windows `--secret-fd 3 -- ping`: refused (`secret_fd_windows`, exit 6, fixed message) before any read or dial.
#[cfg(windows)]
#[test]
fn shell_transport_windows_secret_fd_refused() {
    let fixture = boot("shell_transport_windows_secret_fd_refused");
    write_profile(&fixture);
    let (env_key, env_value) = profile_env(&fixture);
    let run = spawn_direct(
        &fixture,
        shell_argv(&["--secret-fd", "3", "--", "ping"]),
        DirectStdin::Null,
        vec![(env_key, env_value)],
    );
    assert_no_leak(&fixture, &run);
    let record = parse_one_record(&run);
    expect_error(
        &run,
        &record,
        "ping",
        "secret_fd_windows",
        6,
        Some(FD_MESSAGE),
    );
}

/// Unix `--secret-fd 0`: piped fd 0 (never a TTY), NO `--secret-stdin` — the dup(2) path reads the real canary and live TLS ping succeeds. Negatives: never-inherited fd (`invalid_secret`, exit 6, before any read/dial) and >4096 bytes (`secret_too_long`, exit 6, never truncated).
#[cfg(unix)]
#[test]
fn shell_transport_unix_secret_fd_ping() {
    let fixture = boot("shell_transport_unix_secret_fd_ping");
    let path = write_profile(&fixture);
    assert_profile_clean(&fixture, &path);
    let (env_key, env_value) = profile_env(&fixture);
    let fd_ping = |fd: &'static str, payload: Option<Zeroizing<Vec<u8>>>| {
        let stdin = match payload {
            Some(bytes) => DirectStdin::Bytes(bytes),
            None => DirectStdin::Null,
        };
        spawn_direct(
            &fixture,
            shell_argv(&["--secret-fd", fd, "--", "ping"]),
            stdin,
            vec![(env_key, env_value.clone())],
        )
    };

    // Success: canary + LF on fd 0, one v1 result record, no leak.
    let mut payload = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    payload.push(b'\n');
    let run = fd_ping("0", Some(payload));
    assert_no_leak(&fixture, &run);
    assert_eq!(run.exit_code, 0, "fd-0 secret ping must succeed");
    expect_result(&parse_one_record(&run), "ping", Some("Ping"));
    assert_profile_clean(&fixture, &path);
    assert_no_created_files(&fixture);

    // Invalid fd: 200 is never inherited — `dup(2)` fails before any read
    // or dial (no catalogue seed here): typed `invalid_secret`, exit 6.
    let run = fd_ping("200", None);
    assert_no_leak(&fixture, &run);
    let record = parse_one_record(&run);
    expect_error(&run, &record, "ping", "invalid_secret", 6, None);

    // Reader bound: 4097 content bytes ride fd 0 — refused at the cap
    // (`secret_too_long`, exit 6), nothing echoed.
    let mut payload = Zeroizing::new(vec![b'a'; 4097]);
    payload.push(b'\n');
    let run = fd_ping("0", Some(payload));
    assert_no_leak(&fixture, &run);
    let record = parse_one_record(&run);
    expect_error(&run, &record, "ping", "secret_too_long", 6, None);
}

// ── `--secret-env` (design §3.2/§3.3: the fixed variable only) ───────

/// `--secret-env` against the LIVE TLS-TCP server: the canary rides the CHILD's ENVIRONMENT only. Negatives: missing variable (`invalid_secret`, exit 6) and a wrong variable NAME argument (usage, exit 1, before any read; never echoed).
#[test]
fn shell_transport_env_secret_ping() {
    let fixture = boot("shell_transport_env_secret_ping");
    let path = write_profile(&fixture);
    assert_profile_clean(&fixture, &path);
    let (env_key, env_value) = profile_env(&fixture);
    let env_ping = |secret_env_var: Option<&'static str>, env_arg: &'static str| {
        let mut env = vec![(env_key, env_value.clone())];
        // Any inherited value of the fixed variable is removed first.
        if let Some(variable) = secret_env_var {
            env.push((variable, fixture.canary.as_str().to_owned()));
        }
        spawn_direct(
            &fixture,
            shell_argv(&["--secret-env", env_arg, "--", "ping"]),
            DirectStdin::Null,
            env,
        )
    };

    // Success: the real variable carries the canary, the shell reads it,
    // dials TLS-TCP + SCRAM with the verified pin and pings.
    let run = env_ping(Some("SHAMIR_SHELL_PASSWORD"), "SHAMIR_SHELL_PASSWORD");
    assert_no_leak(&fixture, &run);
    assert_eq!(run.exit_code, 0, "env-secret ping must succeed");
    expect_result(&parse_one_record(&run), "ping", Some("Ping"));
    assert_profile_clean(&fixture, &path);
    assert_no_created_files(&fixture);

    // Negative (missing variable): typed `invalid_secret`, exit 6.
    let run = env_ping(None, "SHAMIR_SHELL_PASSWORD");
    assert_no_leak(&fixture, &run);
    let record = parse_one_record(&run);
    expect_error(&run, &record, "ping", "invalid_secret", 6, None);

    // Negative (wrong variable NAME argument): the fixed-name enforcement
    // fires the typed usage refusal (exit 1) BEFORE any read; the real
    // variable IS set and must stay unused; the argument is never echoed.
    let run = env_ping(Some("SHAMIR_SHELL_PASSWORD"), "SHAMIR_SHELL_PASSWORD_OTHER");
    assert_no_leak(&fixture, &run);
    let record = parse_one_record(&run);
    expect_error(&run, &record, "ping", "usage", 1, None);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!stderr.contains("SHAMIR_SHELL_PASSWORD_OTHER"));
}

// ── Output hygiene and malicious-peer reflection ─────────────────────

/// Real wrong-permission / extra-link profile leaves (no crafted wire): typed `insecure_profile` (exit 6) BEFORE any connection. Unix widens the mode off 0600 and adds a second hard link; Windows recreates the leaf with plain `std::fs::write` (default inherited DACL). Each refusal is followed by a restore-and-succeed control on the SAME leaf.
#[test]
fn shell_transport_insecure_profile_leaf_refused() {
    let fixture = boot("shell_transport_insecure_profile_leaf_refused");
    let path = write_profile(&fixture);

    #[cfg(unix)]
    {
        // Unix: widen the mode off 0600, then a real second hard link —
        // each refusal followed by a restore-and-succeed control.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("widen profile leaf mode");
        expect_refusal(&fixture, &["ping"], "ping", "insecure_profile", 6);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("restore profile leaf mode");
        ping_ok(&fixture);

        // Add a REAL second hard link: the count-1 pin refuses; removal
        // restores the control.
        let alias = path.with_file_name("fixture-link.ktav");
        std::fs::hard_link(&path, &alias).expect("create second hard link");
        expect_refusal(&fixture, &["ping"], "ping", "insecure_profile", 6);
        std::fs::remove_file(&alias).expect("remove hard-link alias");
        ping_ok(&fixture);
    }

    #[cfg(windows)]
    {
        // Windows: drop the owner-only leaf and recreate it with plain
        // std::fs::write (default inherited DACL) — the leaf DACL check
        // refuses; the control removes the leaf first (overwrite only
        // truncates) so the atomic owner-only creation restores it.
        let text = std::fs::read_to_string(&path).expect("profile readable");
        std::fs::remove_file(&path).expect("drop protected profile leaf");
        std::fs::write(&path, text.as_bytes())
            .expect("recreate profile leaf with default inherited DACL");
        expect_refusal(&fixture, &["ping"], "ping", "insecure_profile", 6);
        std::fs::remove_file(&path).expect("drop insecure profile leaf");
        overwrite_profile(&fixture, &text);
        ping_ok(&fixture);
    }
}

/// Malicious-peer rule + hygiene on a failing request: only typed code and fixed local message — never peer detail, argv or panic; no history file.
#[test]
fn shell_transport_hygiene_and_reflection() {
    let fixture = boot("shell_transport_hygiene_and_reflection");
    let path = write_profile(&fixture);
    seed_catalogue(&fixture);

    // Failing request: a missing table name the server reflects in its
    // error message — the needle must not come back through the shell.
    let needle = "ghosttable";
    let run = run_shell(&fixture, &["table", "describe", REPO_NAME, needle]);
    assert_no_leak(&fixture, &run);
    let stderr = String::from_utf8_lossy(&run.stderr);
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(!stdout.contains(needle) && !stderr.contains(needle));
    assert!(!stderr.contains(&format!("table describe {REPO_NAME} {needle}")));
    let record = parse_one_record(&run);
    expect_error(&run, &record, "table describe", "not_found", 5, None);

    // Argv hygiene: the canary never travels on argv; profile stays clean.
    assert_profile_clean(&fixture, &path);
    assert_no_created_files(&fixture);
}

// ── Catalogue data reflection, permission denial, scripted interactive ─

/// Byte-scan: no raw C0 control byte except LF.
fn assert_no_raw_controls(bytes: &[u8], what: &str) {
    assert!(
        bytes.iter().all(|byte| *byte >= 0x20 || *byte == b'\n'),
        "{what} carries a raw control byte (peer text must be escaped)"
    );
}

/// Catalogue DATA reflection: `index list` over the REAL odd-name index — raw byte scan, JSON parser, DECODED equality against [`INDEX_ODD_ESCAPED`].
#[test]
fn shell_transport_catalogue_data_reflection() {
    let fixture = boot("shell_transport_catalogue_data_reflection");
    let path = write_profile(&fixture);
    seed_catalogue(&fixture);
    let run = run_shell_ok(&fixture, &["index", "list", REPO_NAME, INDEXED_TABLE]);
    assert_no_leak(&fixture, &run);
    assert_no_raw_controls(&run.stdout, "batch stdout");
    let record = parse_one_record(&run);
    expect_result(&record, "index list", Some("List"));
    let names: Vec<&str> = record
        .pointer("/data/indexes")
        .and_then(Value::as_array)
        .expect("indexes")
        .iter()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .collect();
    // The record carries the ESCAPED name only: the projected renderer
    // escapes it, so the RAW odd-name bytes must never survive the scan.
    assert!(
        !names.contains(&INDEX_ODD_NAME),
        "raw odd name must not survive"
    );
    assert!(names.contains(&INDEX_ODD_ESCAPED), "name must be escaped");
    assert_profile_clean(&fixture, &path);
    assert_no_created_files(&fixture);
}

/// Real non-superuser denial (§5 exit 4): the limited principal authenticates (positive `ping` control, leak-checked) but the catalogue probe is typed `permission_denied`, exit 4. The admin control on the SAME server runs BEFORE its credentials are replaced.
#[test]
fn shell_transport_non_superuser_denied() {
    let fixture = boot("shell_transport_non_superuser_denied");
    let path = write_profile(&fixture);
    seed_catalogue(&fixture);
    let user_secret = seed_shell_user(&fixture);
    // ADMIN positive control FIRST — the profile still carries admin
    // credentials here, so this run proves the server itself serves the
    // catalogue probe. The limited-credential profile replaces it after.
    let run = run_shell_ok(&fixture, &["db", "list"]);
    assert_no_leak(&fixture, &run);
    let text = pinned_profile(
        &fixture,
        "tls-tcp",
        &fixture.tls_addr.to_string(),
        Some("localhost"),
        DB_NAME,
    )
    .replacen("username: admin\n", &format!("username: {SHELL_USER}\n"), 1);
    // The limited profile never carries the user's secret.
    assert!(
        !text.contains(user_secret.as_str()),
        "secret in profile text"
    );
    overwrite_profile(&fixture, &text);
    let (env_key, env_value) = profile_env(&fixture);
    let user_run = |args: &[&'static str]| {
        let mut argv = shell_argv(&["--secret-stdin", "--"]);
        argv.extend(args.iter().copied());
        // The user's REAL secret must never travel assembled argv (the
        // fixture canary is checked inside spawn_direct; this is the
        // second, distinct zeroized secret).
        assert!(
            !argv.iter().any(|arg| arg.contains(user_secret.as_str())),
            "secret on argv"
        );
        let mut payload = Zeroizing::new(user_secret.as_bytes().to_vec());
        payload.push(b'\n');
        spawn_direct(
            &fixture,
            argv,
            DirectStdin::Bytes(payload),
            vec![(env_key, env_value.clone())],
        )
    };
    // Positive auth control: the limited principal SCRAMs and pings —
    // leak-checked on SUCCESS as well as on the later denial.
    let run = user_run(&["ping"]);
    assert_no_leak(&fixture, &run);
    assert!(
        !run.contains_canary(user_secret.as_str()),
        "user secret leaked on success"
    );
    assert_eq!(run.exit_code, 0, "limited principal authenticates");
    expect_result(&parse_one_record(&run), "ping", Some("Ping"));
    // The denial: typed, exit 4, no server text, no secret echo.
    let run = user_run(&["db", "list"]);
    assert_no_leak(&fixture, &run);
    assert!(
        !run.contains_canary(user_secret.as_str()),
        "user secret leaked"
    );
    let record = parse_one_record(&run);
    expect_error(
        &run,
        &record,
        "db list",
        "permission_denied",
        4,
        Some(DENIAL_MESSAGE),
    );
    assert_no_created_files(&fixture);
    assert_profile_clean(&fixture, &path);
}

/// Scripted fake terminal: one scripted secret read, restore always Ok — local copy (the output suite's fake is not visible); payload returned zeroized.
struct ScriptedTerminal {
    read: Result<Zeroizing<Vec<u8>>, ShellError>,
}
impl Terminal for ScriptedTerminal {
    fn begin_hidden_read(&self) -> Result<Box<dyn HiddenLineSession + '_>, ShellError> {
        Ok(Box::new(ScriptedSession {
            read: self.read.clone(),
        }))
    }
}
struct ScriptedSession {
    read: Result<Zeroizing<Vec<u8>>, ShellError>,
}
impl HiddenLineSession for ScriptedSession {
    fn read_line(&mut self) -> Result<Zeroizing<Vec<u8>>, ShellError> {
        self.read.clone()
    }
    fn restore(self: Box<Self>) -> Result<(), ShellError> {
        Ok(())
    }
}

/// Successful scripted INTERACTIVE path against the REAL seeded server: the real REPL via `run_scripted`, fake terminal credential (canary rides the fake, never argv), fixture profiles root. EOF ends the session; human tables land on the injected stderr. Escaped-name oracle is truncation-invariant; full equality lives in the batch reflection test.
#[test]
fn shell_transport_scripted_interactive() {
    let fixture = boot("shell_transport_scripted_interactive");
    let path = write_profile(&fixture);
    seed_catalogue(&fixture);
    let terminal = ScriptedTerminal {
        read: Ok(Zeroizing::new(fixture.canary.as_bytes().to_vec())),
    };
    let script = format!("ping\nindex list {REPO_NAME} {INDEXED_TABLE}\n");
    let mut input = Cursor::new(script.into_bytes());
    let mut stderr: Vec<u8> = Vec::new();
    fixture.guard.mark("shell: scripted interactive");
    // `run_scripted` takes the args AFTER the leading `shell` token (the
    // seam itself prepends `shell` before the clap parse).
    let exit = run_scripted(
        ["--profile".into(), PROFILE_NAME.into()],
        &mut input,
        &mut stderr,
        &terminal,
        Some(path.parent().expect("fixture profiles root")),
    );
    assert_eq!(exit, 0, "scripted interactive session must succeed");
    // Human tables go to the injected stderr: both command heads ran (a
    // failed command would carry the fixed error block instead).
    let human = String::from_utf8_lossy(&stderr);
    assert!(
        human.contains("ping"),
        "interactive stderr must contain the ping success table"
    );
    assert!(
        human.contains("index list"),
        "stderr must contain the index list table"
    );
    assert!(
        !human.contains("error"),
        "a failed command renders the fixed error block"
    );
    // Escaped name re-rendered (JSON-doubled then \u005C); the escape-boundary
    // truncation of the 200-char cap always keeps the head fragment.
    assert!(
        human.contains(INDEX_ODD_HUMAN_FRAGMENT),
        "escaped name fragment must appear"
    );
    // Full name when the cell fits, else the escape-safe truncation marker.
    assert!(human.contains(INDEX_ODD_HUMAN_FULL) || human.contains("..."));
    assert_no_raw_controls(&stderr, "interactive stderr");
    assert_profile_clean(&fixture, &path);
    assert_no_created_files(&fixture);
}
