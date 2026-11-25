//! Shared fixtures, helpers and the counting relay for the R5 PR3
//! state/data shell tests (the sibling `goldens`, `token_subset` and
//! `subscriptions` modules). One primary item per file is kept there;
//! THIS file owns the shared helpers only.
//!
//! Every network operation uses a real fixture over pinned TLS/IPC/WSS and
//! bounded handshakes; wire writes use the typed query builders. Insert
//! responses decode as `QueryRecord::Direct` maps; the storage ID is read from
//! the `_id` field.

use std::net::SocketAddr;
use std::time::Duration;

use serde_json::Value;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_types::types::value::QueryValue;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use zeroize::Zeroizing;

use super::harness::{profile_text, ShellFixture, ShellRun, DB_NAME, PLAIN_TABLE, REPO_NAME};

/// Hard hang bound — a hang DETECTOR, not a latency assertion.
pub(super) const BOUND: Duration = Duration::from_secs(120);

/// Wall-clock lifetime of ONE counting relay. It must outlive every SERIAL
/// child it fronts — the token-refusal probes run seven commands, each with its
/// own [`BOUND`] — while [`BOUND`] still bounds each individual step. A relay
/// that shared a single [`BOUND`] would fail its own join on a slow runner
/// even though every child stayed inside its bound.
pub(super) const RELAY_LIFETIME: Duration = Duration::from_secs(900);

/// The fixed token-subset refusal message (§3.3; identical to the PR2
/// `refusals` suite — the message is fixed, not per-command).
pub(super) const TOKEN_SUBSET: &str =
    "token sessions expose only the read-only shell subset; log in with SCRAM for this command";

/// Top-level fields of the `status` payload (`data` object of
/// `shell/commands/state.rs::project_status`) — the §4.2 contract.
pub(super) const STATUS_FIELDS: &[&str] = &[
    "server_version",
    "format_epoch",
    "stamp_outcome",
    "uptime_ms",
    "role",
    "node_id",
    "gap_caveat",
    "age_caveat",
    "replication",
    "repositories",
];

/// `StatusStampOutcome` renderings of `shell/commands/state.rs`
/// (snake_case).
pub(super) const STAMP_OUTCOMES: &[&str] = &[
    "created",
    "adopted",
    "unchanged",
    "version_updated",
    "inspected_stampless",
    "inspected_older",
];

// ── Output contract helpers ──────────────────────────────────────────

/// The `data` object of a successful single-record JSON Lines v1 result.
/// Asserts exactly one LF-terminated line, wire version 1, exit 0, a
/// nonempty command id whose lowercase form contains `command_part`, and
/// `kind: result`.
pub(super) fn data(run: &ShellRun, command_part: &str) -> Value {
    assert_eq!(run.exit_code, 0, "shell command failed (exit code only)");
    let line = run
        .stdout
        .strip_suffix(b"\n")
        .expect("LF-terminated output");
    assert!(!line.contains(&b'\n'), "expected exactly one record");
    let value: Value = serde_json::from_slice(line).unwrap_or_else(|_| panic!("invalid JSON"));
    assert_eq!(value["version"].as_u64(), Some(1), "wrong wire version");
    let command = value["command"].as_str().unwrap_or_default();
    assert!(
        command.to_ascii_lowercase().contains(command_part),
        "wrong command id for `{command_part}`"
    );
    assert!(
        value["kind"].as_str() == Some("result"),
        "expected result kind"
    );
    assert!(run.stderr.is_empty(), "unexpected batch diagnostics");
    value["data"].clone()
}

/// The ERROR ENVELOPE of a typed single-record refusal: `exit` per the §5
/// table, `kind: error`, the expected `code`, a nonempty message. The
/// WHOLE envelope is returned (callers assert on `message` and on the
/// `data` leaf — always `null` for errors — through this value).
pub(super) fn refusal(run: &ShellRun, command_part: &str, code: &str, exit: i32) -> Value {
    assert_eq!(
        run.exit_code, exit,
        "wrong exit for `{command_part}` ({code})"
    );
    let line = run
        .stdout
        .strip_suffix(b"\n")
        .expect("LF-terminated output");
    assert!(!line.contains(&b'\n'), "expected exactly one record");
    let value: Value = serde_json::from_slice(line).unwrap_or_else(|_| panic!("invalid JSON"));
    assert_eq!(value["version"].as_u64(), Some(1), "wrong wire version");
    let command = value["command"].as_str().unwrap_or_default();
    assert!(
        command.to_ascii_lowercase().contains(command_part),
        "wrong command id for `{command_part}`"
    );
    assert!(
        value["kind"].as_str() == Some("error"),
        "expected error kind"
    );
    assert_eq!(value["code"].as_str(), Some(code), "wrong refusal code");
    assert!(
        value["message"].as_str().is_some_and(|m| !m.is_empty()),
        "empty refusal message"
    );
    assert!(
        value["data"].is_null(),
        "an error envelope never carries data"
    );
    value
}

/// Read a leaf as u64 from the documented decimal-STRING rendering
/// (defensively accepting a JSON number too).
pub(super) fn leaf_u64(leaf: &Value) -> Option<u64> {
    match leaf {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    }
}

/// `true` when the output text carries the standalone word `lag`
/// (case-insensitive) — the gap line must say `gap_versions_merged` with
/// its merged-aggregate caveat, never "lag".
pub(super) fn mentions_lag(run: &ShellRun) -> bool {
    let text = String::from_utf8_lossy(&run.stdout).to_ascii_lowercase();
    text.split(|c: char| !c.is_ascii_alphabetic())
        .any(|word| word == "lag")
}

// ── Seeding helpers ──────────────────────────────────────────────────

/// Insert `rows` real records into the fixture's plain table over the REAL
/// pinned wire (builder-only), returning the server-minted base58
/// `RecordId` strings in insertion order. Field `n` carries the row index.
pub(super) fn insert_rows(fixture: &ShellFixture, rows: usize) -> Vec<String> {
    fixture.guard.mark("seed: state_data rows");
    let addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    fixture.rt.run(async move {
        let client = timeout(
            BOUND,
            super::harness::connect_tls_owned(addr, pin, password),
        )
        .await
        .expect("seed connect hang bound")
        .expect("seed verified TLS connect");
        let mut batch = Batch::new();
        batch.id("state_data_seed");
        batch.insert(
            "ins",
            shamir_query_builder::write::Insert::with_repo(REPO_NAME, PLAIN_TABLE).rows(
                (0..rows).map(|n| {
                    doc! { "n" => i64::try_from(n).unwrap_or(0), "tag" => "state_data_probe" }
                }),
            ),
        );
        let response = timeout(BOUND, client.execute(DB_NAME, batch.build()))
            .await
            .expect("seed insert hang bound")
            .expect("seed insert executes");
        let result = response
            .results
            .get("ins")
            .expect("insert result alias present");
        // ORACLE: the server serialises a write-result row as a msgpack map
        // carrying the storage id under `_id` (Base58 string), and the client
        // always decodes a map as `QueryRecord::Direct` (query_record.rs).
        let ids: Vec<String> = result
            .records
            .iter()
            .map(|record| {
                record
                    .get_value_str("_id")
                    .expect("server-minted `_id` on inserted row")
                    .to_owned()
            })
            .collect();
        assert_eq!(ids.len(), rows, "insert echoes one id per row");
        timeout(BOUND, client.close())
            .await
            .expect("seed close hang bound");
        ids
    })
}

/// Insert TWO real records with DISTINCT stored values (and distinct `n`)
/// into the fixture's plain table over the REAL pinned wire, returning
/// their server-minted Base58 `RecordId` strings as `(first, second)`.
/// The point-read golden needs both: a reply that carries the second row
/// must never satisfy an assertion written for the first.
pub(super) fn insert_distinct_pair(fixture: &ShellFixture) -> (String, String) {
    fixture.guard.mark("seed: state_data distinct pair");
    let addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    fixture.rt.run(async move {
        let client = timeout(
            BOUND,
            super::harness::connect_tls_owned(addr, pin, password),
        )
        .await
        .expect("distinct-pair connect hang bound")
        .expect("distinct-pair verified TLS connect");
        let mut batch = Batch::new();
        batch.id("state_data_distinct_pair");
        batch.insert(
            "ins",
            shamir_query_builder::write::Insert::with_repo(REPO_NAME, PLAIN_TABLE).rows([
                doc! { "n" => 101_i64, "tag" => "state_data_first" },
                doc! { "n" => 202_i64, "tag" => "state_data_second" },
            ]),
        );
        let response = timeout(BOUND, client.execute(DB_NAME, batch.build()))
            .await
            .expect("distinct-pair insert hang bound")
            .expect("distinct-pair insert executes");
        let result = response
            .results
            .get("ins")
            .expect("insert result alias present");
        // ORACLE: same decode path as `insert_rows` — the server serialises
        // a write-result row as a msgpack map carrying the storage id under
        // `_id` (Base58 string).
        let ids: Vec<String> = result
            .records
            .iter()
            .map(|record| {
                record
                    .get_value_str("_id")
                    .expect("server-minted `_id` on inserted row")
                    .to_owned()
            })
            .collect();
        assert_eq!(ids.len(), 2, "insert echoes one id per inserted row");
        timeout(BOUND, client.close())
            .await
            .expect("distinct-pair close hang bound");
        let [first, second] = ids.as_slice() else {
            panic!("distinct-pair insert did not echo exactly two ids");
        };
        (first.clone(), second.clone())
    })
}

/// Insert ONE real record with the given string field (builder-only
/// wire), returning its server-minted Base58 id. Used by the hostile-value
/// and credential-reflection probes; authorized defensive content only.
pub(super) fn insert_doc(fixture: &ShellFixture, field: &str, value: &str) -> String {
    fixture.guard.mark("seed: hostile doc");
    let addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    let field = field.to_owned();
    let value = Zeroizing::new(value.to_owned());
    fixture.rt.run(async move {
        let client = timeout(
            BOUND,
            super::harness::connect_tls_owned(addr, pin, password),
        )
        .await
        .expect("hostile-doc connect hang bound")
        .expect("hostile-doc verified TLS connect");
        let mut fields = shamir_collections::new_map();
        fields.insert(String::from("n"), QueryValue::Int(0));
        fields.insert(field, QueryValue::Str(value.to_string()));
        let mut batch = Batch::new();
        batch.id("state_data_hostile_doc");
        batch.insert(
            "ins",
            shamir_query_builder::write::Insert::with_repo(REPO_NAME, PLAIN_TABLE)
                .rows([QueryValue::Map(fields)]),
        );
        let response = timeout(BOUND, client.execute(DB_NAME, batch.build()))
            .await
            .expect("hostile-doc insert hang bound")
            .expect("hostile-doc insert executes");
        let result = response
            .results
            .get("ins")
            .expect("insert result alias present");
        let [record] = result.records.as_slice() else {
            panic!("hostile-doc insert did not echo one inserted row");
        };
        let id = record
            .get_value_str("_id")
            .expect("server-minted `_id` on inserted row")
            .to_owned();
        timeout(BOUND, client.close())
            .await
            .expect("hostile-doc close hang bound");
        id
    })
}

/// The profile text for an admin SCRAM transport, with the fixture's
/// verified pin filled in.
pub(super) fn scram_profile(fixture: &ShellFixture, transport: &str, address: &str) -> String {
    let sni = (transport != "ipc").then_some("localhost");
    profile_text(transport, address, sni, Some(DB_NAME))
        .replace("pin: <PIN>\n", &format!("pin: {}\n", fixture.pin_hex()))
}

/// One subprocess owns a fresh counting relay: it counts the FIRST
/// downstream connection (0 or 1 falsifies pre-dial), forwarding TLS
/// unchanged. Same shape as the PR2 `refusals` relay (that one is
/// module-private; visibility is not widened for this file).
pub(super) struct Relay {
    pub(super) addr: SocketAddr,
    stop: oneshot::Sender<()>,
    task: JoinHandle<usize>,
}

impl Relay {
    pub(super) fn start(fixture: &ShellFixture, target: SocketAddr) -> Self {
        fixture.rt.run(async move {
            let listener = timeout(BOUND, TcpListener::bind("127.0.0.1:0"))
                .await
                .expect("relay bind hang bound")
                .expect("relay bind failed");
            let addr = listener.local_addr().expect("relay bound address");
            let (stop, mut stopped) = oneshot::channel();
            let task = tokio::spawn(async move {
                timeout(RELAY_LIFETIME, async {
                    let accepted = tokio::select! {
                        accepted = listener.accept() => Some(accepted),
                        _ = &mut stopped => None,
                    };
                    let Some(accepted) = accepted else {
                        // The child has exited. Check the kernel's accept
                        // queue directly, even if Tokio has not yet
                        // delivered the readiness event.
                        let listener = listener.into_std().expect("convert relay listener");
                        return match listener.accept() {
                            Ok(_) => 1,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => 0,
                            Err(_) => panic!("relay accept queue check failed"),
                        };
                    };
                    let mut downstream = accepted.expect("relay accept failed").0;
                    tokio::select! {
                        _ = &mut stopped => {},
                        () = async {
                            let mut upstream = TcpStream::connect(target)
                                .await
                                .expect("relay upstream connect failed");
                            let _ =
                                tokio::io::copy_bidirectional(&mut downstream, &mut upstream)
                                    .await;
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

    pub(super) fn finish(self, fixture: &ShellFixture) -> usize {
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
