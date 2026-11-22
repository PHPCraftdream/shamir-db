//! R5 PR3 shell state/data tests: `status`/`version show` goldens, the empty subscriptions fixture, the data count/head/get goldens and refusals, and the no-role denial sweep. Drives the REAL production
//! shell through the shared helpers of [`super::state_data_common`].

use serde_json::Value;
use shamir_sync::format_stamp::{inspect, StampState};
use tokio::net::TcpListener;

use super::harness::{
    overwrite_profile, run_shell, seed_catalogue, seed_shell_user, DB_NAME, PLAIN_TABLE, REPO_NAME,
    SCHEMA_TABLE, SHELL_USER,
};
use super::secure_fixture::{boot_secure, run_with_secret};
use super::state_data_common::{
    data, insert_distinct_pair, insert_doc, insert_rows, leaf_u64, mentions_lag, refusal,
    scram_profile, STAMP_OUTCOMES, STATUS_FIELDS,
};

// ── Tests ────────────────────────────────────────────────────────────

/// `status` over every SCRAM transport: golden structure at STABLE field
/// locations — `data.server_version` / `format_epoch` (decimal string) /
/// `stamp_outcome` / `uptime_ms` (decimal string) / `role`, the fixed
/// `gap_caveat` and `age_caveat` texts, an EMPTY `replication` array (the
/// standalone fixture has no replication peer) and exactly the one seeded
/// repository with its `wal` block. The epoch is verified
/// against the ACTUAL stamp file of the fixture data directory
/// (`format_stamp::inspect`), and `version show` renders the SAME epoch
/// and outcome.
#[test]
fn status_and_version_show_golden_structure() {
    let secure = boot_secure("status_and_version_show_golden_structure");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    // The ground truth for the launcher-observed stamp: the fixture data
    // directory ON DISK (read-only inspect; the fixture is a fresh boot,
    // so the stamp is valid and stamped at the current epoch).
    let stamp_epoch = match inspect(super::secure_fixture::data_dir(fixture)) {
        StampState::Valid(stamp) => stamp.format_epoch(),
        other => panic!("fixture stamp is not valid on disk: {other:?}"),
    };
    for (transport, addr) in [
        ("tls-tcp", fixture.tls_addr.to_string()),
        ("wss", secure.wss_addr.to_string()),
        ("ipc", fixture.ipc_addr.clone()),
    ] {
        fixture.guard.mark(format!("status over {transport}"));
        overwrite_profile(fixture, &scram_profile(fixture, transport, &addr));
        let run = run_shell(fixture, &["status"]);
        assert!(
            !run.contains_canary(fixture.canary.as_str()),
            "shell secret leaked ({transport})"
        );
        assert!(
            !mentions_lag(&run),
            "status output says `lag` ({transport})"
        );
        let payload = data(&run, "status");
        // ENVELOPE NODE ORACLE (§5/§6): the `node` field of the status
        // envelope carries the OBSERVED role (a standalone ReadWrite node
        // is `unknown`) — never an all-null object. This fails when
        // `observed_node` is removed from `shell/output.rs`.
        let envelope: Value = serde_json::from_slice(
            run.stdout
                .strip_suffix(b"\n")
                .expect("LF-terminated output"),
        )
        .expect("status envelope is valid JSON");
        assert_eq!(
            envelope["node"]["role"].as_str(),
            Some("unknown"),
            "status envelope `node` lost the observed role"
        );
        assert_eq!(
            envelope["node"]["node_id"], payload["node_id"],
            "status envelope `node` diverged from the observed node id"
        );
        let object = payload
            .as_object()
            .expect("status payload is a JSON object");
        for field in STATUS_FIELDS {
            assert!(
                object.contains_key(*field),
                "status missing field `{field}`"
            );
        }
        let epoch = leaf_u64(&payload["format_epoch"])
            .unwrap_or_else(|| panic!("format_epoch is not a decimal string"));
        assert_eq!(
            epoch, stamp_epoch,
            "status epoch is not the fixture's actual on-disk stamp epoch"
        );
        assert!(
            STAMP_OUTCOMES.contains(&payload["stamp_outcome"].as_str().unwrap_or_default()),
            "unknown stamp_outcome rendering"
        );
        // Presence and decimal-string rendering only: a freshly booted
        // fixture may legitimately report uptime_ms == 0 (monotonic clock
        // at process start), so no positivity threshold is asserted here.
        leaf_u64(&payload["uptime_ms"])
            .unwrap_or_else(|| panic!("uptime_ms is not a decimal string"));
        // Exact observed role: the fixture launches a ReadWrite node, so
        // the launcher's configured mode renders as `unknown` (standalone;
        // the engine `operator_state.rs` maps NodeMode::ReadWrite →
        // StatusRole::Unknown). Not merely nonempty — EXACT.
        assert_eq!(
            payload["role"].as_str(),
            Some("unknown"),
            "a standalone ReadWrite fixture must report role `unknown`"
        );
        assert!(
            payload["server_version"]
                .as_str()
                .is_some_and(|v| !v.is_empty()),
            "server_version missing or empty"
        );
        // The fixed caveats: the gap is named and non-authoritative; the
        // age is explicitly not sampled ("zero can mean not sampled").
        let gap_caveat = payload["gap_caveat"].as_str().unwrap_or_default();
        assert!(
            gap_caveat.contains("gap_versions_merged") && gap_caveat.contains("non-authoritative"),
            "gap_caveat lost the named-field, non-authoritative wording"
        );
        let age_caveat = payload["age_caveat"].as_str().unwrap_or_default();
        assert!(
            age_caveat.contains("zero can mean not sampled"),
            "age_caveat lost the not-sampled wording"
        );
        // Replication rows: the standalone fixture has NO replication peer
        // configured, so the exported merged-gap snapshot is an EMPTY
        // array — asserted exactly, never loosely.
        let replication = payload["replication"]
            .as_array()
            .expect("replication array");
        assert!(
            replication.is_empty(),
            "a standalone fixture must report no replication rows"
        );
        // Durability / WAL telemetry per repository: the seeded catalogue
        // repository appears with its identity, and its `wal` block is
        // present. The server boot also opens its default database and
        // repository, so the list is searched by identity, never indexed.
        let repositories = payload["repositories"]
            .as_array()
            .expect("repositories array");
        let seeded = repositories
            .iter()
            .filter(|row| {
                row["db"].as_str() == Some(DB_NAME) && row["repo"].as_str() == Some(REPO_NAME)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            seeded.len(),
            1,
            "the seeded repository appears exactly once"
        );
        assert!(
            seeded[0].get("wal").is_some(),
            "repository telemetry missing its `wal` block"
        );
        let text = serde_json::to_string(&payload).expect("status payload serializes");
        assert!(
            text.to_ascii_lowercase().contains("wal"),
            "status missing WAL telemetry"
        );

        // `version show` is a VIEW of the same observed stamp: identical
        // epoch and outcome, no new disk read.
        let version_run = run_shell(fixture, &["version", "show"]);
        assert!(
            !version_run.contains_canary(fixture.canary.as_str()),
            "shell secret leaked (version show {transport})"
        );
        let version = data(&version_run, "version");
        let version_envelope: Value = serde_json::from_slice(
            version_run
                .stdout
                .strip_suffix(b"\n")
                .expect("LF-terminated output"),
        )
        .expect("version envelope is valid JSON");
        assert_eq!(
            version_envelope["node"]["role"].as_str(),
            Some("unknown"),
            "version show envelope `node` lost the observed role"
        );
        assert_eq!(
            version_envelope["node"]["node_id"], version["node_id"],
            "version envelope `node` diverged from the observed node id"
        );
        assert_eq!(
            leaf_u64(&version["format_epoch"]),
            Some(stamp_epoch),
            "version show epoch differs from the fixture stamp"
        );
        assert_eq!(
            version["stamp_outcome"].as_str(),
            payload["stamp_outcome"].as_str(),
            "version show outcome differs from status"
        );
    }

    // Uptime is monotonic across two successive live queries (monotonic
    // clock only — NO wall-clock threshold, which would be a flake).
    overwrite_profile(
        fixture,
        &scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string()),
    );
    let first = leaf_u64(&data(&run_shell(fixture, &["status"]), "status")["uptime_ms"])
        .expect("first uptime_ms");
    let second = leaf_u64(&data(&run_shell(fixture, &["status"]), "status")["uptime_ms"])
        .expect("second uptime_ms");
    assert!(
        second >= first,
        "uptime_ms regressed between two status calls"
    );
}

/// `replication subscriptions` through the existing batch-path admin op:
/// with no subscription configured on the fixture it succeeds and reports
/// an empty list (a configured-subscription golden needs a replication
/// peer fixture and is reported as deferred coverage).
#[test]
fn replication_subscriptions_empty_fixture() {
    let secure = boot_secure("replication_subscriptions_empty_fixture");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    overwrite_profile(
        fixture,
        &scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string()),
    );
    let run = run_shell(fixture, &["replication", "subscriptions"]);
    assert!(
        !run.contains_canary(fixture.canary.as_str()),
        "shell secret leaked"
    );
    let payload = data(&run, "subscriptions");
    // The settled projection shape (§4.2): the root carries EXACTLY the
    // `subscriptions` list, and the unconfigured fixture reports an EMPTY
    // one — asserted exactly, not via a shape-agnostic scan.
    assert_eq!(
        payload.as_object().map(|o| o.len()),
        Some(1),
        "subscriptions payload must carry exactly one field"
    );
    assert_eq!(
        payload["subscriptions"].as_array().map(Vec::len),
        Some(0),
        "unconfigured fixture reported subscriptions"
    );
}

/// `data count` / `data head` / `data get` goldens against REAL inserted
/// rows, plus the typed refusals at their intended codes and §5 exits:
/// `data get` with a malformed KEY is `invalid_record_id` (exit 1,
/// PRE-NETWORK — the profile points at a dead endpoint, so a dial would
/// surface `connection_failed` instead), a missing table is
/// `unknown_table` (exit 5), and the typed point-read miss is
/// `not_found` (exit 5).
#[test]
fn data_count_head_get_goldens_and_refusals() {
    let secure = boot_secure("data_count_head_get_goldens_and_refusals");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    let ids = insert_rows(fixture, 7);
    overwrite_profile(
        fixture,
        &scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string()),
    );

    // data count: the exact inserted row count at data.count, decimal
    // string, plus a count-path report (never confused with metadata).
    let count_run = run_shell(fixture, &["data", "count", DB_NAME, REPO_NAME, PLAIN_TABLE]);
    assert!(!count_run.contains_canary(fixture.canary.as_str()));
    let count_payload = data(&count_run, "count");
    assert_eq!(
        count_payload["count"].as_str(),
        Some("7"),
        "data.count must render the exact inserted row count as a decimal string"
    );
    assert!(
        count_payload["count_path"]
            .as_str()
            .is_some_and(|p| !p.is_empty()),
        "data count lost its count_path report"
    );

    // data head 5: exactly 5 rows at data.rows, every row one of the
    // seeded rows (matched by its `n` payload field), NO ordering
    // promise — the comparison is a SET check, never order-dependent.
    let head_run = run_shell(
        fixture,
        &["data", "head", DB_NAME, REPO_NAME, PLAIN_TABLE, "5"],
    );
    assert!(!head_run.contains_canary(fixture.canary.as_str()));
    let head_payload = data(&head_run, "head");
    let rows = head_payload["rows"].as_array().expect("data.rows array");
    assert_eq!(rows.len(), 5, "data head did not return exactly N rows");
    // Both budget markers are present on every head result (§4.4).
    assert_eq!(
        head_payload["rows_truncated"].as_bool(),
        Some(false),
        "an uncapped small head must not claim row truncation"
    );
    assert_eq!(
        head_payload["bytes_truncated"].as_bool(),
        Some(false),
        "an uncapped small head must not claim byte truncation"
    );
    let mut seen: Vec<i64> = Vec::new();
    for row in rows {
        let n = leaf_u64(&row["n"])
            .map(|n| i64::try_from(n).unwrap_or(-1))
            .unwrap_or_else(|| panic!("head row without its `n` payload field"));
        assert!(
            (0..7).contains(&n),
            "data head returned a row the fixture never inserted"
        );
        assert!(!seen.contains(&n), "data head returned the same row twice");
        seen.push(n);
    }

    // data head beyond the shell-side 100-row cap: requested 150, the
    // clamp is visible (data.rows_capped) and exactly 100 rows come back.
    insert_rows(fixture, 120);
    let capped_run = run_shell(
        fixture,
        &["data", "head", DB_NAME, REPO_NAME, PLAIN_TABLE, "150"],
    );
    assert!(!capped_run.contains_canary(fixture.canary.as_str()));
    let capped = data(&capped_run, "head");
    assert_eq!(
        capped["rows_capped"].as_bool(),
        Some(true),
        "a request above the 100-row cap must carry data.rows_capped"
    );
    assert_eq!(
        capped["rows_returned"].as_str(),
        Some("100"),
        "the row clamp must return exactly the cap"
    );
    assert!(
        capped["rows"]
            .as_array()
            .is_some_and(|rows| rows.len() == 100),
        "data.rows must carry exactly the capped row count"
    );
    // The clamp stopped on the ROW cap, not the byte budget: the row
    // truncation marker is set and the byte-truncation marker stays false.
    assert_eq!(
        capped["rows_truncated"].as_bool(),
        Some(true),
        "a request above the 100-row cap must carry data.rows_truncated"
    );
    assert_eq!(
        capped["bytes_truncated"].as_bool(),
        Some(false),
        "a row-clamped head must not claim byte truncation"
    );

    // data get with a REAL inserted id: the projection is EXACTLY the
    // requested row — asserted UNCONDITIONALLY, no "any record will do"
    // fallback. The two rows seeded for this probe carry DISTINCT stored
    // values (`n` 101/202, tags `state_data_first`/`state_data_second`),
    // so a reply carrying the OTHER row fails the assertions below.
    // `real_id` stays the first row of `insert_rows` for the later typed
    // not_found / unknown_table probes.
    let real_id = &ids[0];
    let (first_id, _second_id) = insert_distinct_pair(fixture);
    let get_run = run_shell(
        fixture,
        &["data", "get", DB_NAME, REPO_NAME, PLAIN_TABLE, &first_id],
    );
    assert!(!get_run.contains_canary(fixture.canary.as_str()));
    let got = data(&get_run, "get");
    let got_object = got.as_object().expect("data get payload is a JSON object");
    let mut keys = got_object.keys().map(String::as_str).collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["n", "tag"],
        "data get must project exactly the seeded row's fields"
    );
    assert_eq!(
        leaf_u64(&got["n"]),
        Some(101),
        "data get returned a DIFFERENT record than the requested first row"
    );
    assert_eq!(
        got["tag"].as_str(),
        Some("state_data_first"),
        "data get lost the requested row's payload value"
    );
    // The OTHER seeded row must never satisfy this golden: a value-only
    // comparison is not enough, so assert EXPLICITLY that the reply does
    // not carry the second row's distinct values.
    let second_payload = serde_json::json!({ "n": 202, "tag": "state_data_second" });
    let got_text = serde_json::to_string(&got).expect("data get payload serializes");
    let second_text = serde_json::to_string(&second_payload).expect("second row serializes");
    assert!(
        got_text != second_text && !got_text.contains("state_data_second"),
        "data get returned the SECOND seeded row instead of the requested one"
    );
    assert_ne!(
        leaf_u64(&got["n"]),
        Some(202),
        "data get returned the SECOND seeded row's distinct `n`"
    );

    // HOSTILE-VALUE probe (real fixture record, real output): a record
    // whose payload carries a C0 control byte must NEVER reach stdout
    // raw — the v1 sanitizer escapes it (`\u0007`), and the raw byte is
    // absent from the output stream. This fails if the projection
    // sanitizer is removed.
    let hostile_id = insert_doc(fixture, "note", "\u{0007}bell\u{001b}[31m");
    let hostile_run = run_shell(
        fixture,
        &["data", "get", DB_NAME, REPO_NAME, PLAIN_TABLE, &hostile_id],
    );
    assert!(!hostile_run.contains_canary(fixture.canary.as_str()));
    assert_eq!(
        hostile_run.exit_code, 0,
        "a hostile payload is a success, not a refusal"
    );
    assert!(
        !hostile_run.stdout.contains(&0x07) && !hostile_run.stdout.contains(&0x1b),
        "raw control bytes reached stdout"
    );
    let hostile_text = String::from_utf8_lossy(&hostile_run.stdout);
    assert!(
        hostile_text.contains("\\u0007") && hostile_text.contains("\\u001B"),
        "the sanitizer did not escape the hostile control bytes"
    );

    // not_found (typed P2 miss, exit 5): a REAL id addressed in ANOTHER
    // table (valid format, absent key — the schema table is empty here).
    let wrong_table_run = run_shell(
        fixture,
        &["data", "get", DB_NAME, REPO_NAME, SCHEMA_TABLE, real_id],
    );
    assert!(!wrong_table_run.contains_canary(fixture.canary.as_str()));
    refusal(&wrong_table_run, "get", "not_found", 5);

    // A missing table: the typed `unknown_table` refusal with exit 5 is
    // carried by the P2 point read (the engine emits the code there only);
    // the batch `count` path refuses as a generic server error, exit 5 —
    // never a usage error and never a crash.
    let get_missing = run_shell(
        fixture,
        &["data", "get", DB_NAME, REPO_NAME, "no_such_table", real_id],
    );
    assert!(!get_missing.contains_canary(fixture.canary.as_str()));
    refusal(&get_missing, "data", "unknown_table", 5);
    let count_missing = run_shell(
        fixture,
        &["data", "count", DB_NAME, REPO_NAME, "no_such_table"],
    );
    assert!(!count_missing.contains_canary(fixture.canary.as_str()));
    refusal(&count_missing, "data", "server_error", 5);

    // malformed KEY: typed `invalid_record_id` (exit 1), PRE-NETWORK. The
    // profile points at a bound-then-dropped port; a dial would fail with
    // `connection_failed` (exit 2), never the typed key code.
    let dead = fixture.rt.run(async move {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("dead endpoint bind");
        listener.local_addr().expect("dead endpoint address")
    });
    overwrite_profile(
        fixture,
        &scram_profile(fixture, "tls-tcp", &dead.to_string()),
    );
    let malformed = run_shell(
        fixture,
        &[
            "data",
            "get",
            DB_NAME,
            REPO_NAME,
            PLAIN_TABLE,
            "definitely!!not~base58",
        ],
    );
    assert!(!malformed.contains_canary(fixture.canary.as_str()));
    refusal(&malformed, "get", "invalid_record_id", 1);
}

/// A user with NO roles gets the typed `permission_denied` (exit 4) for
/// every state/data command over SCRAM — the two-call Database-then-Table
/// authorization refuses before any lookup.
#[test]
fn no_role_user_permission_denied() {
    let secure = boot_secure("no_role_user_permission_denied");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    let ids = insert_rows(fixture, 1);
    let user_password = seed_shell_user(fixture);
    let text = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string())
        .replace("username: admin\n", &format!("username: {SHELL_USER}\n"));
    overwrite_profile(fixture, &text);
    for args in [
        vec!["status"],
        vec!["version", "show"],
        vec!["data", "count", DB_NAME, REPO_NAME, PLAIN_TABLE],
        vec!["data", "head", DB_NAME, REPO_NAME, PLAIN_TABLE, "3"],
        vec![
            "data",
            "get",
            DB_NAME,
            REPO_NAME,
            PLAIN_TABLE,
            ids[0].as_str(),
        ],
        vec!["replication", "subscriptions"],
    ] {
        fixture
            .guard
            .mark(format!("limited user: {}", args.join(" ")));
        let run = run_with_secret(fixture, user_password.clone(), false, &args);
        assert!(
            !run.contains_canary(fixture.canary.as_str())
                && !run.contains_canary(user_password.as_str()),
            "secret leaked in limited-user run"
        );
        let command_part = match args[0] {
            "version" => "version",
            "replication" => "subscriptions",
            "data" => "data",
            _ => "status",
        };
        let value = refusal(&run, command_part, "permission_denied", 4);
        // A typed refusal must not echo the probed record id.
        let text = serde_json::to_string(&value).expect("refusal serializes");
        assert!(
            !text.contains(ids[0].as_str()),
            "refusal echoed the probed record id"
        );
    }
}
