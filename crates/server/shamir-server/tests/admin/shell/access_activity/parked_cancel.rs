//! A genuinely PARKED request cancelled through the real shell.
//!
//! Topology: raw-SCRAM session A as the holder (`conn1`) — it seeds the row,
//! opens a pessimistic interactive transaction and stages a row write (which
//! holds the row's Level-3 record lock). A second raw-SCRAM session of the
//! same user (`conn2`) issues the conflicting pessimistic write; the server
//! parks that request as `waiting_lock`. The production shell runs as
//! bootstrap admin (session B): it observes the park through
//! `activity requests --all` and cancels it with `activity cancel <id> --yes`.
//!
//! Every await is hang-bounded by [`BOUND`]; the park is confirmed by a
//! bounded poll loop with no sleeps (each iteration runs the real shell, so
//! the loop yields to the runtime and to the parked request).

use std::future::Future;
use std::time::Duration;

use serde_json::Value;
use shamir_query_builder::batch::{Batch, Isolation};
use shamir_query_builder::{doc, filter, write};
use shamir_query_types::admin::ResourceRef;
use shamir_query_types::batch::{BatchResponse, IsolationLevel};
use shamir_query_types::write::UpdateOp;
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_server::version::CURRENT_QUERY_LANG_VERSION;
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use zeroize::Zeroizing;

use crate::permission_e2e::common::{roundtrip, scram_login};

use super::harness::{
    boot, overwrite_profile, run_shell, seed_catalogue, seed_shell_user, ShellFixture, ShellRun,
    ADMIN, DB_NAME, PLAIN_TABLE, REPO_NAME, SHELL_USER,
};
use super::secure_fixture::set_acl_mode;
use super::state_data_common::{data, scram_profile};

/// Hard hang bound — a hang DETECTOR, not a latency assertion.
const BOUND: Duration = Duration::from_secs(120);

// ── Local shape helpers ──────────────────────────────────────────────
//
// Test-support copies, kept private to this module on purpose: the
// activity tests own the originals and nothing here is shared with them.

/// Fields of one projected request row (`project_request`).
const REQUEST_FIELDS: &[&str] = &[
    "kind",
    "request_exec_id",
    "session_handle",
    "user",
    "db",
    "op_kind",
    "state",
    "elapsed_ns",
];

/// The documented request-state renderings (`request_state_name`).
const REQUEST_STATES: &[&str] = &[
    "running",
    "done",
    "committing",
    "cancelled",
    "waiting_lock",
    "waiting_visibility",
    "waiting_budget",
];

/// Exactly the `expected` top-level keys of `value` — no missing, no extra.
fn keys_exact(value: &Value, expected: &[&str], context: &str) {
    let fields = value
        .as_object()
        .unwrap_or_else(|| panic!("{context} is not a JSON object"));
    let mut actual = fields.keys().map(String::as_str).collect::<Vec<_>>();
    actual.sort_unstable();
    let mut wanted = expected.to_vec();
    wanted.sort_unstable();
    assert_eq!(actual, wanted, "{context} field set");
}

// ── Raw session transport ────────────────────────────────────────────
//
// The SDK's `tx_begin`/`tx_execute`/`tx_rollback` helpers are gone; the
// sessions below speak the wire directly through the raw-SCRAM support the
// `permission_e2e` tests already own (`scram_login` + `roundtrip`), so the
// bytes on the wire and the assertions on the replies are the same as the
// old `Client` calls.

/// Response side of one raw session's TLS transport.
type TlsRead = ReadHalf<TlsStream<TcpStream>>;
/// Request side of one raw session's TLS transport.
type TlsWrite = WriteHalf<TlsStream<TcpStream>>;

/// One live raw-SCRAM session: its server-minted id, the running request id
/// and its split TLS halves.
struct RawSession {
    /// Session id minted by SCRAM `auth_ok` (never logged, never printed).
    sid: [u8; 32],
    /// Next request id to stamp on the envelope.
    next_rid: u32,
    /// Response side of the session transport.
    read: TlsRead,
    /// Request side of the session transport.
    write: TlsWrite,
}

impl RawSession {
    /// One bounded request/reply on this session; hands the session back so
    /// the same connection carries the next call.
    fn ask(mut self, fixture: &ShellFixture, req: DbRequest) -> (Self, DbResponse) {
        fixture.rt.run(async move {
            let response = exchange(&mut self, &req).await;
            (self, response)
        })
    }
}

/// Open one raw session as `SHELL_USER` (the password bytes are zeroized on
/// drop, like the old `ConnectOptions` carried them).
fn login(fixture: &ShellFixture, password: &[u8]) -> RawSession {
    let addr = fixture.tls_addr;
    let password = Zeroizing::new(password.to_vec());
    // The `within` bound is built INSIDE the runtime job: the timeout's
    // timer handle comes from the runtime context.
    let (sid, read, write, next_rid) = fixture
        .rt
        .run(async move { within(scram_login(addr, SHELL_USER, &password)).await });
    RawSession {
        sid,
        next_rid,
        read,
        write,
    }
}

/// One bounded request/reply on a borrowed session (also used from the
/// spawned task, which owns its session for the whole call).
async fn exchange(session: &mut RawSession, req: &DbRequest) -> DbResponse {
    within(roundtrip(
        req,
        session.sid,
        &mut session.next_rid,
        &mut session.write,
        &mut session.read,
    ))
    .await
}

/// Unwrap one successful batch-shaped reply; any other shape is a failure of
/// `context` (an `Error` reply names its server-side code).
fn batch_reply(response: DbResponse, context: &str) -> BatchResponse {
    match response {
        DbResponse::Batch { response } | DbResponse::TxBatch { response } => response,
        DbResponse::Error { code, .. } => panic!("{context}: server replied `{code}`"),
        _ => panic!("{context}: unexpected reply shape"),
    }
}

// ── Fixture helpers ──────────────────────────────────────────────────

/// Point the shell's profile at the bootstrap admin over TLS-TCP.
fn use_admin_profile(fixture: &ShellFixture) {
    let text = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string());
    overwrite_profile(fixture, &text);
}

/// One bounded await; an elapse is a genuine hang.
async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(BOUND, future)
        .await
        .expect("step exceeded the 120 s hang bound")
}

/// Grant A's no-role account owner-independent read/write on the catalogue
/// path; authorization stays a real server-side ACL check.
fn grant_write_access(fixture: &ShellFixture) {
    set_acl_mode(
        fixture,
        ResourceRef::Database {
            database: DB_NAME.into(),
        },
        0o777,
    );
    set_acl_mode(
        fixture,
        ResourceRef::Store {
            store: [DB_NAME.into(), REPO_NAME.into()],
        },
        0o777,
    );
    set_acl_mode(
        fixture,
        ResourceRef::Table {
            table: [DB_NAME.into(), REPO_NAME.into(), PLAIN_TABLE.into()],
        },
        0o777,
    );
}

/// Insert the one row A's interactive transaction will lock.
fn insert_target_row(fixture: &ShellFixture, session: RawSession) -> RawSession {
    let mut batch = Batch::new();
    batch.id("seed_target");
    batch.insert(
        "row",
        write::Insert::with_repo(REPO_NAME, PLAIN_TABLE).rows([doc! { "n" => 1_i64 }]),
    );
    let request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB_NAME.into(),
        batch: batch.build(),
    };
    let (session, reply) = session.ask(fixture, request);
    let response = batch_reply(reply, "seed target row executes");
    let inserted = response
        .results
        .get("row")
        .expect("seed target alias present");
    assert_eq!(
        inserted.records.len(),
        1,
        "the target insert must echo one row"
    );
    session
}

/// The one conflicting update both transactions target.
fn update_n(from: i64, to: i64) -> UpdateOp {
    write::Update::with_repo(REPO_NAME, PLAIN_TABLE)
        .where_(filter::eq("n", from))
        .set(doc! { "n" => to })
        .build()
        .expect("fixture update")
}

/// Begin A's pessimistic interactive transaction.
fn begin_pessimistic_tx(fixture: &ShellFixture, session: RawSession) -> (RawSession, u64) {
    let request = DbRequest::TxBegin {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB_NAME.into(),
        repo: REPO_NAME.into(),
        isolation: Some(IsolationLevel::Pessimistic),
    };
    let (session, reply) = session.ask(fixture, request);
    match reply {
        DbResponse::TxOpened { tx_handle, .. } => (session, tx_handle),
        DbResponse::Error { code, .. } => {
            panic!("session A begins its interactive transaction: server replied `{code}`")
        }
        _ => panic!("session A begins its interactive transaction: unexpected reply shape"),
    }
}

/// Stage the row write inside A's open transaction; the staged write keeps
/// the row's record lock until commit or rollback.
fn stage_write(fixture: &ShellFixture, session: RawSession, tx_handle: u64) -> RawSession {
    let mut batch = Batch::new();
    batch.id("stage");
    batch.update("staged", update_n(1, -1));
    let request = DbRequest::TxExecute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB_NAME.into(),
        tx_handle,
        batch: batch.build(),
    };
    let (session, reply) = session.ask(fixture, request);
    let response = batch_reply(reply, "staged write executes inside the transaction");
    let staged = response
        .results
        .get("staged")
        .expect("staged alias present");
    let stats = staged
        .stats
        .as_ref()
        .expect("the staged update result carries stats");
    assert_eq!(
        stats.records_returned, 1,
        "the staged update must hit one row"
    );
    session
}

/// Spawn A's second, conflicting request. It blocks on the row lock the open
/// transaction already holds, so the server parks it as `waiting_lock`.
fn spawn_parked_request(
    fixture: &ShellFixture,
    mut session: RawSession,
) -> tokio::task::JoinHandle<DbResponse> {
    let mut batch = Batch::new();
    batch.id("parked");
    batch.transactional().isolation(Isolation::Pessimistic);
    batch.update("blocked", update_n(1, 2));
    let request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB_NAME.into(),
        batch: batch.build(),
    };
    // The task owns its session until the server answers, exactly like the
    // old `tokio::spawn(client.execute(...))`.
    fixture
        .rt
        .run(async move {
            (tokio::spawn(async move {
                exchange(&mut session, &request).await
            }),)
        })
        .0
}

/// Roll back A's open transaction under a hang bound.
fn rollback_tx(fixture: &ShellFixture, session: RawSession, tx_handle: u64) -> RawSession {
    let request = DbRequest::TxRollback {
        db: DB_NAME.into(),
        tx_handle,
    };
    let (session, reply) = session.ask(fixture, request);
    match reply {
        DbResponse::TxRolledBack {
            tx_handle: rolled_back,
        } => assert_eq!(
            rolled_back, tx_handle,
            "session A rolls back the handle it opened"
        ),
        DbResponse::Error { code, .. } => {
            panic!("session A rolls back its transaction: server replied `{code}`")
        }
        _ => panic!("session A rolls back its transaction: unexpected reply shape"),
    }
    session
}

/// Close one raw session: a bounded TLS `close_notify` on the write half
/// replaces `Client::close`, then both halves drop and the socket closes.
fn close_session(fixture: &ShellFixture, session: RawSession) {
    let RawSession {
        mut write, read, ..
    } = session;
    fixture.rt.run(async move {
        within(write.shutdown())
            .await
            .expect("session A close hang bound");
        drop(read);
    });
}

// ── Output and leak helpers ──────────────────────────────────────────

/// Run the real shell and refuse any captured byte stream carrying session
/// material. The failure text never embeds the output or the needle.
fn checked_shell(fixture: &ShellFixture, needles: &[&str], args: &[&str]) -> ShellRun {
    let run = run_shell(fixture, args);
    for needle in needles {
        assert!(
            !run.contains_canary(needle),
            "session material leaked in a shell byte stream"
        );
    }
    run
}

fn bytes_contain(haystack: &[u8], needle: &str) -> bool {
    needle.len() <= haystack.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
}

/// Recursive post-run invariant: no entry NAME and no file BYTES under the
/// operator's profile home carry the canary or A's session id.
fn scan_profile_home(fixture: &ShellFixture, needles: &[&str]) {
    let mut dirs = vec![fixture.profile_home.path().to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("read profile home") {
            let entry = entry.expect("profile home entry");
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy().into_owned();
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

/// Read one documented decimal-string leaf without echoing its value.
fn decimal(value: &Value, context: &str) -> String {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("{context} is not a decimal string"));
    assert!(
        !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()),
        "{context} is not a decimal string"
    );
    text.to_owned()
}

// ── Parked-row observation ───────────────────────────────────────────

/// Bounded poll of `activity requests --all` until A's parked request is
/// listed. Each iteration runs the real shell, so it yields to the runtime
/// and to the parked request; there are no sleeps.
fn await_parked_row(fixture: &ShellFixture, needles: &[&str]) -> Value {
    let deadline = std::time::Instant::now() + BOUND;
    loop {
        let run = checked_shell(fixture, needles, &["activity", "requests", "--all"]);
        let payload = data(&run, "requests");
        assert_eq!(payload["scope"], "all", "`--all` must widen the scope");
        let rows = payload["rows"]
            .as_array()
            .expect("activity requests `rows` is not an array");
        let mut parked = None;
        for row in rows {
            keys_exact(row, REQUEST_FIELDS, "activity requests row");
            let state = row["state"]
                .as_str()
                .expect("request row `state` is not a string");
            assert!(
                REQUEST_STATES.contains(&state),
                "request row `state` is not a documented rendering"
            );
            if state == "waiting_lock" {
                parked = Some(row.clone());
            }
        }
        if let Some(row) = parked {
            return row;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no A request was listed as waiting_lock within the 120 s bound"
        );
    }
}

/// Prove, from the same `activity sessions --all` page, that `parked_handle`
/// is one of A's live sessions (A holds TWO raw sessions: the transaction
/// holder and the parked victim) and return the shell's OWN admin handle.
/// Handles are minted monotonically, so the listing shell's (newest) admin
/// session is the maximum admin handle on the page.
fn shell_own_handle(fixture: &ShellFixture, needles: &[&str], parked_handle: &str) -> String {
    let run = checked_shell(fixture, needles, &["activity", "sessions", "--all"]);
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

// ── Test ─────────────────────────────────────────────────────────────

/// A request parked on A's own record lock is listed with the exact request
/// field set and a foreign session handle, then cancelled through the real
/// shell: the shell reports the documented `cancelled` outcome and A's
/// request fails with the typed `cancelled` error. A's session id and the
/// bootstrap canary never reach the shell's streams or the profile home.
///
/// Mutants caught:
/// 1. A listing filtered to the caller's own session: `--all` would never
///    surface A's parked row and [`await_parked_row`] fails at its bound.
/// 2. A cancel that reports success without the server having accepted it:
///    the parked future would not resolve with the typed `cancelled` error.
/// 3. Any projection that echoes the live session id or the secret: the
///    per-run stream scan and the profile-home scan both reject it.
#[test]
fn parked_request_is_cancelled_through_the_shell() {
    let fixture = boot("parked_request_is_cancelled_through_the_shell");
    let fixture = &fixture;
    seed_catalogue(fixture);
    let a_password = seed_shell_user(fixture);
    grant_write_access(fixture);
    use_admin_profile(fixture);

    let conn1 = login(fixture, a_password.as_bytes());
    let conn2 = login(fixture, a_password.as_bytes());
    let session_hex = hex::encode(conn2.sid);
    let needles = [
        fixture.canary.as_str(),
        a_password.as_str(),
        session_hex.as_str(),
    ];

    fixture.guard.mark("park: seed the target row");
    let conn1 = insert_target_row(fixture, conn1);

    fixture
        .guard
        .mark("park: A opens a pessimistic transaction and stages a write");
    let (conn1, tx_handle) = begin_pessimistic_tx(fixture, conn1);
    let conn1 = stage_write(fixture, conn1, tx_handle);

    fixture.guard.mark("park: A issues the conflicting write");
    let parked = spawn_parked_request(fixture, conn2);

    fixture
        .guard
        .mark("park: shell observes the parked request");
    let row = await_parked_row(fixture, &needles);

    // ── Assertion 1: the parked row's shape, identity and isolation ──
    let parked_id = decimal(&row["request_exec_id"], "parked request_exec_id");
    let parked_handle = decimal(&row["session_handle"], "parked session_handle");
    assert_eq!(row["kind"], "request", "wrong row kind in a requests list");
    assert_eq!(
        row["state"], "waiting_lock",
        "the request must be listed as parked on a record lock"
    );
    assert_eq!(row["user"], SHELL_USER, "the parked request's user");
    assert_eq!(row["db"], DB_NAME, "the parked request's database");
    decimal(&row["elapsed_ns"], "parked elapsed_ns");
    assert!(
        row["op_kind"].as_str().is_some_and(|kind| !kind.is_empty()),
        "the parked request must carry a rendered op_kind"
    );
    let shell_handle = shell_own_handle(fixture, &needles, &parked_handle);
    assert_ne!(
        parked_handle, shell_handle,
        "the parked row belongs to A's session, never the shell's own"
    );

    let own_run = checked_shell(fixture, &needles, &["activity", "requests"]);
    let own = data(&own_run, "requests");
    assert_eq!(own["scope"], "own", "the default scope must be own");
    for own_row in own["rows"].as_array().expect("own rows array") {
        assert_ne!(
            own_row["request_exec_id"].as_str(),
            Some(parked_id.as_str()),
            "another user's request must not appear in the own scope"
        );
    }

    // ── Assertion 2: documented cancel outcome and typed failure ─────
    fixture.guard.mark("park: shell cancels the parked request");
    let cancel_run = checked_shell(
        fixture,
        &needles,
        &["activity", "cancel", &parked_id, "--yes"],
    );
    let cancel = data(&cancel_run, "cancel");
    keys_exact(&cancel, &["outcome"], "activity cancel");
    assert_eq!(
        cancel["outcome"], "cancelled",
        "the server must accept the cancellation"
    );

    let joined = fixture.rt.run(async move { within(parked).await });
    let reply = match joined {
        Ok(reply) => reply,
        Err(_) => panic!("the parked request task did not join"),
    };
    match reply {
        DbResponse::Error { code, .. } => assert_eq!(
            code, "cancelled",
            "the parked request must fail with the typed `cancelled` code"
        ),
        DbResponse::Batch { .. } => panic!("the cancelled request unexpectedly completed"),
        _other => panic!("the parked request failed with an unexpected typed error"),
    }

    // ── Assertion 3: no session material in streams or the profile home ─
    fixture.guard.mark("park: leak scan");
    scan_profile_home(fixture, &needles);

    fixture.guard.mark("park: rollback and close session A");
    let conn1 = rollback_tx(fixture, conn1, tx_handle);
    close_session(fixture, conn1);
}
