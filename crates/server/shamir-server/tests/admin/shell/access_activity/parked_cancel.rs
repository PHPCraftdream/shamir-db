//! A genuinely PARKED request cancelled through the real shell.
//!
//! Topology: one real [`shamir_client::Client`] session A and the production
//! shell as bootstrap admin (session B). A opens a pessimistic interactive
//! transaction, stages a row write (which holds the row's Level-3 record
//! lock), and then issues a conflicting pessimistic write. The server parks
//! that second A request as `waiting_lock`; the shell observes it through
//! `activity requests --all` and cancels it with `activity cancel <id> --yes`.
//!
//! Every await is hang-bounded by [`BOUND`]; the park is confirmed by a
//! bounded poll loop with no sleeps (each iteration runs the real shell, so
//! the loop yields to the runtime and to the parked request).

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use shamir_client::{BatchResponse, Client, ClientError, ConnectOptions};
use shamir_query_builder::batch::{Batch, Isolation};
use shamir_query_builder::{doc, filter, write};
use shamir_query_types::admin::ResourceRef;
use shamir_query_types::batch::IsolationLevel;
use shamir_query_types::write::UpdateOp;
use zeroize::Zeroizing;

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

// ── Fixture helpers ──────────────────────────────────────────────────

/// Point the shell's profile at the bootstrap admin over TLS-TCP.
fn use_admin_profile(fixture: &ShellFixture) {
    let text = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string());
    overwrite_profile(fixture, &text);
}

/// Connect one REAL second session as `SHELL_USER` and keep it open.
fn open_shell_user_session(fixture: &ShellFixture, password: Zeroizing<String>) -> Client {
    let addr = fixture.tls_addr;
    let pin = fixture.pin;
    fixture.rt.run(async move {
        // `ConnectOptions` carries the password as bytes (zeroized on drop).
        let password = Zeroizing::new(password.as_bytes().to_vec());
        let options = ConnectOptions {
            addr,
            server_name: String::from("localhost"),
            username: String::from(SHELL_USER),
            password,
            accept_new_host: false,
            trusted_pin: Some(pin),
            connect_timeout: Some(BOUND),
            handshake_timeout: Some(BOUND),
            request_timeout: Some(BOUND),
        };
        tokio::time::timeout(BOUND, Client::connect(options))
            .await
            .expect("second-session connect hang bound")
            .expect("second-session verified TLS connect")
    })
}

/// Close one live session under a hang bound (`close` awaits the transport
/// shutdown).
fn close_session(fixture: &ShellFixture, client: Client) {
    fixture.rt.run(async move {
        tokio::time::timeout(BOUND, client.close())
            .await
            .expect("second-session close hang bound");
    });
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
fn insert_target_row(fixture: &ShellFixture, client: &Arc<Client>) {
    let client = Arc::clone(client);
    fixture.rt.run(async move {
        let mut batch = Batch::new();
        batch.id("seed_target");
        batch.insert(
            "row",
            write::Insert::with_repo(REPO_NAME, PLAIN_TABLE).rows([doc! { "n" => 1_i64 }]),
        );
        let response = within(client.execute(DB_NAME, batch.build()))
            .await
            .expect("seed target row executes");
        let inserted = response
            .results
            .get("row")
            .expect("seed target alias present");
        assert_eq!(
            inserted.records.len(),
            1,
            "the target insert must echo one row"
        );
    });
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
fn begin_pessimistic_tx(fixture: &ShellFixture, client: &Arc<Client>) -> u64 {
    let client = Arc::clone(client);
    fixture.rt.run(async move {
        within(client.tx_begin(DB_NAME, REPO_NAME, Some(IsolationLevel::Pessimistic)))
            .await
            .expect("session A begins its interactive transaction")
    })
}

/// Stage the row write inside A's open transaction; the staged write keeps
/// the row's record lock until commit or rollback.
fn stage_write(fixture: &ShellFixture, client: &Arc<Client>, tx_handle: u64) {
    let client = Arc::clone(client);
    fixture.rt.run(async move {
        let mut batch = Batch::new();
        batch.id("stage");
        batch.update("staged", update_n(1, -1));
        let response = within(client.tx_execute(DB_NAME, tx_handle, batch.build()))
            .await
            .expect("staged write executes inside the transaction");
        let staged = response
            .results
            .get("staged")
            .expect("staged alias present");
        assert_eq!(
            staged.records.len(),
            1,
            "the staged update must hit one row"
        );
    });
}

/// Spawn A's second, conflicting request. It blocks on the row lock the open
/// transaction already holds, so the server parks it as `waiting_lock`.
fn spawn_parked_request(
    fixture: &ShellFixture,
    client: &Arc<Client>,
) -> tokio::task::JoinHandle<Result<BatchResponse, ClientError>> {
    let client = Arc::clone(client);
    fixture
        .rt
        .run(async move {
            (tokio::spawn(async move {
                let mut batch = Batch::new();
                batch.id("parked");
                batch.transactional().isolation(Isolation::Pessimistic);
                batch.update("blocked", update_n(1, 2));
                within(client.execute(DB_NAME, batch.build())).await
            }),)
        })
        .0
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
/// is A's live session and return the shell's OWN admin handle. Handles are
/// minted monotonically, so the listing shell's (newest) admin session is the
/// maximum admin handle on the page.
fn shell_own_handle(fixture: &ShellFixture, needles: &[&str], parked_handle: &str) -> String {
    let run = checked_shell(fixture, needles, &["activity", "sessions", "--all"]);
    let payload = data(&run, "sessions");
    assert_eq!(payload["scope"], "all", "`--all` must widen the scope");
    let rows = payload["rows"]
        .as_array()
        .expect("activity sessions `rows` is not an array");
    let foreign_rows: Vec<&Value> = rows
        .iter()
        .filter(|row| row["user"].as_str() == Some(SHELL_USER))
        .collect();
    assert_eq!(
        foreign_rows.len(),
        1,
        "the page must carry exactly A's live session"
    );
    let a_handle = decimal(&foreign_rows[0]["session_handle"], "session A handle");
    assert_eq!(
        a_handle, parked_handle,
        "the parked request row belongs to A's live session"
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
    assert_ne!(
        shell_handle, a_handle,
        "A and the shell are distinct sessions"
    );
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

    let client = Arc::new(open_shell_user_session(fixture, a_password.clone()));
    let session_hex = hex::encode(client.session_id());
    let needles = [
        fixture.canary.as_str(),
        a_password.as_str(),
        session_hex.as_str(),
    ];

    fixture.guard.mark("park: seed the target row");
    insert_target_row(fixture, &client);

    fixture
        .guard
        .mark("park: A opens a pessimistic transaction and stages a write");
    let tx_handle = begin_pessimistic_tx(fixture, &client);
    stage_write(fixture, &client, tx_handle);

    fixture.guard.mark("park: A issues the conflicting write");
    let parked = spawn_parked_request(fixture, &client);

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
    let result = match joined {
        Ok(result) => result,
        Err(_) => panic!("the parked request task did not join"),
    };
    match result {
        Err(ClientError::Db { code, .. }) => assert_eq!(
            code, "cancelled",
            "the parked request must fail with the typed `cancelled` code"
        ),
        Err(_other) => panic!("the parked request failed with an unexpected typed error"),
        Ok(_) => panic!("the cancelled request unexpectedly completed"),
    }

    // ── Assertion 3: no session material in streams or the profile home ─
    fixture.guard.mark("park: leak scan");
    scan_profile_home(fixture, &needles);

    fixture.guard.mark("park: rollback and close session A");
    let rollback = Arc::clone(&client);
    fixture.rt.run(async move {
        within(rollback.tx_rollback(DB_NAME, tx_handle))
            .await
            .expect("session A rolls back its transaction")
    });
    let client = match Arc::try_unwrap(client) {
        Ok(client) => client,
        Err(_) => panic!("session A is still shared at teardown"),
    };
    close_session(fixture, client);
}
