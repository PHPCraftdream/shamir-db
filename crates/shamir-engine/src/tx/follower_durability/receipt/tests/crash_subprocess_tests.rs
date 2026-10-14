//! Kill-subprocess crash harness (S5c-X PR6b-2, review CI requirement): a
//! `#[tokio::test]` child crashes the process with `std::process::abort()`
//! inside `test_seam::phase()` at a receipt pipeline phase; the PARENT tests
//! spawn the child through the libtest binary, then reopen the repo and
//! replay the SAME batch — asserting retry convergence: identical history
//! allocation, byte-identical journal rows across both replays, absolute
//! counters, and a retired receipt with an advanced bookmark v3.
//!
//! The only permitted wait is the bounded fjall Database-lock release retry
//! right after the child was killed (resource acquisition, not
//! synchronization).

use bytes::Bytes;
use shamir_storage::types::{RecordKey, Repo as _, Store};
use shamir_tx::{decode_version_key, ChangeOp, ChangelogEvent, RecordChange};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::super::activation::disable_receipt_lookup;
use crate::meta::repl::bookmark_v3::load_bookmark_v3;
use crate::meta::repl::receipt::receipt_key;
use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;
use crate::tx::apply_replicated::apply_replicated_batch;
use crate::tx::apply_replicated::follower_durability::test_seam::{self, Phase};
use crate::tx::{ApplyPolicy, ReplicatedEvent};

/// Full libtest path of the child scenario test in this binary.
const CHILD_TEST: &str = "tx::apply_replicated::follower_durability::receipt::tests::\
                         crash_subprocess_tests::receipt_crash_child_scenario";

// ---------------------------------------------------------------- helpers

fn event(leader_version: u64, id: u8) -> ReplicatedEvent {
    ReplicatedEvent {
        event: ChangelogEvent {
            repo: "leader".into(),
            commit_version: leader_version,
            tx_id: leader_version,
            actor: Actor::System,
            timestamp_ns: 0,
            changes: vec![RecordChange {
                table: "items".into(),
                key: RecordId([id; 16]).to_bytes(),
                op: ChangeOp::Put,
                value: Some(InnerValue::Str(format!("row-{id}")).to_bytes().unwrap()),
            }],
        },
        field_bindings: Vec::new(),
    }
}

fn delete_event(leader_version: u64, id: u8) -> ReplicatedEvent {
    ReplicatedEvent {
        event: ChangelogEvent {
            repo: "leader".into(),
            commit_version: leader_version,
            tx_id: leader_version,
            actor: Actor::System,
            timestamp_ns: 0,
            changes: vec![RecordChange {
                table: "items".into(),
                key: RecordId([id; 16]).to_bytes(),
                op: ChangeOp::Delete,
                value: None,
            }],
        },
        field_bindings: Vec::new(),
    }
}

fn repo_name(path: &Path) -> String {
    format!(
        "receipt-crash-{}",
        path.display().to_string().replace('\\', "_")
    )
}

async fn open_with_retry(path: &Path, name: &str) -> RepoInstance {
    // Bounded resource-acquisition retry: the killed child's fjall Database
    // lock may take a moment to be released by the OS (Windows especially).
    for attempt in 0..25u32 {
        match RepoInstance::from_factory(
            name.to_owned(),
            BoxRepoFactory::fjall_raw(path),
            vec![TableConfig::new("items")],
        )
        .await
        {
            Ok(repo) => return repo,
            Err(error) if attempt < 24 => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("reopen failed after retries: {error}"),
        }
    }
    unreachable!("retry loop always returns or panics")
}

async fn close(repo: RepoInstance) {
    let handle = repo.changefeed().await.unwrap();
    handle.feed.shutdown_and_wait().await.unwrap();
    drop(handle);
    drop(repo);
}

async fn journal_bytes(repo: &RepoInstance, version: u64) -> Option<Bytes> {
    repo.follower_journal_store()
        .await
        .unwrap()
        .get(RecordKey::from_slice(&version.to_be_bytes()))
        .await
        .ok()
}

/// All history rows of `keys`, sorted: `(key, version, value)`.
async fn history_rows(repo: &RepoInstance, keys: &[Vec<u8>]) -> Vec<(Vec<u8>, u64, Bytes)> {
    let store = repo.repo.store_get("__history__items").await.unwrap();
    let mut rows = Vec::new();
    for batch in store.iter_stream(128).collect::<Vec<_>>().await {
        for (record_key, value) in batch.unwrap() {
            if let Some((key, version)) = decode_version_key(record_key.as_ref()) {
                if keys.iter().any(|k| k.as_slice() == key) {
                    rows.push((key.to_vec(), version, value));
                }
            }
        }
    }
    rows.sort();
    rows
}

// ---------------------------------------------------------------- scenario

#[derive(Clone, Copy)]
struct Scenario {
    multi: bool,
    tombstone: bool,
}

impl Scenario {
    fn events(&self) -> Vec<ReplicatedEvent> {
        if self.tombstone {
            // A later event deletes the earlier key.
            vec![event(1, 7), delete_event(2, 7)]
        } else if self.multi {
            // Repeated key: the same key lands at versions 1 and 2, which
            // exercises the ABSOLUTE counter (two puts, one distinct key).
            vec![event(1, 1), event(2, 1)]
        } else {
            vec![event(1, 1)]
        }
    }

    fn keys(&self) -> Vec<Vec<u8>> {
        if self.tombstone {
            vec![RecordId([7; 16]).to_bytes().to_vec()]
        } else if self.multi {
            vec![RecordId([1; 16]).to_bytes().to_vec()]
        } else {
            vec![RecordId([1; 16]).to_bytes().to_vec()]
        }
    }

    fn leader_watermark(&self) -> u64 {
        self.events().len() as u64
    }

    /// Expected `(key, version, value)` history set: one row per event, at
    /// the receipt's local versions `1..=N`; a Delete row is the empty value.
    fn expected_history(&self) -> Vec<(Vec<u8>, u64, Bytes)> {
        self.events()
            .iter()
            .enumerate()
            .map(|(index, event)| {
                let change = &event.event.changes[0];
                let value = match &change.value {
                    Some(value) => Bytes::copy_from_slice(value),
                    None => Bytes::new(),
                };
                (change.key.clone(), index as u64 + 1, value)
            })
            .collect()
    }

    /// Expected absolute row count after the batch.
    fn expected_counter(&self) -> u64 {
        self.events()
            .iter()
            .filter(|event| event.event.changes[0].op == ChangeOp::Put)
            .count() as u64
    }
}

async fn assert_converged(repo: &RepoInstance, scenario: &Scenario) {
    let keys = scenario.keys();
    let expected = scenario.expected_history();
    let versions: Vec<u64> = {
        let mut versions = expected
            .iter()
            .map(|(_, version, _)| *version)
            .collect::<Vec<_>>();
        versions.sort_unstable();
        versions
    };

    // Allocation unchanged: EXACTLY the receipt's versions, no extras.
    assert_eq!(
        history_rows(repo, &keys).await,
        expected,
        "history allocation must be exactly the receipt's versions {versions:?}"
    );

    // Journal bytes at every applied version, non-empty.
    for version in 1..=scenario.leader_watermark() {
        let bytes = journal_bytes(repo, version)
            .await
            .unwrap_or_else(|| panic!("journal row for version {version} missing"));
        assert!(!bytes.is_empty(), "journal row {version} empty");
    }

    // Absolute counter.
    let counter = repo.get_table("items").await.unwrap().counter().clone();
    assert_eq!(
        counter.get().await.unwrap(),
        scenario.expected_counter(),
        "counter must equal the absolute applied row count"
    );

    // Tombstone scenario: the deleted key reads as absent from the snapshot.
    if scenario.tombstone {
        let table = repo.get_table("items").await.unwrap();
        assert!(table.get_bytes(RecordId([7; 16])).await.unwrap().is_none());
        drop(table);
    }

    // Bookmark v3: receipt retired, bookmark == leader watermark,
    // clean horizon covers the batch, receipt key absent.
    let info = repo.tx_info_store().await.unwrap();
    let v3 = load_bookmark_v3(&info).await.unwrap().unwrap();
    assert_eq!(v3.outstanding_receipt, None, "receipt must be retired");
    assert_eq!(v3.bookmark, scenario.leader_watermark());
    assert_eq!(v3.clean_through, Some(scenario.events().len() as u64));
    assert!(matches!(
        info.get(receipt_key()).await,
        Err(shamir_storage::error::DbError::NotFound(_))
    ));
    drop(info);
}

// ------------------------------------------------------------------ child

/// CHILD process scenario: crashes with `std::process::abort()` inside the
/// receipt pipeline at `SHAMIR_RECEIPT_CRASH_POINT`. Returns immediately (a
/// no-op pass) when `SHAMIR_RECEIPT_CRASH_SCENARIO` is unset, so ordinary
/// test runs never spawn anything.
#[tokio::test]
async fn receipt_crash_child_scenario() {
    let Ok(scenario) = std::env::var("SHAMIR_RECEIPT_CRASH_SCENARIO") else {
        return;
    };
    assert_eq!(scenario, "receipt-crash");
    let path = PathBuf::from(std::env::var("SHAMIR_RECEIPT_CRASH_PATH").unwrap());
    let point = std::env::var("SHAMIR_RECEIPT_CRASH_POINT").unwrap();
    let phase = match point.as_str() {
        "prepared" => Phase::ReceiptPrepared,
        "materialized" => Phase::ReceiptMaterialized,
        "checkpointed" => Phase::ReceiptCheckpointed,
        other => panic!("unknown crash point {other}"),
    };

    // NOTE: the crash itself must run through the RECEIPT pipeline (that is
    // the state whose convergence we verify), so the child NEVER disables
    // the receipt lookup here — even when the negative-control harness passes
    // SHAMIR_RECEIPT_CRASH_LOOKUP_DISABLED=1. That env is honored by the
    // PARENT for its in-process replay only.
    let name = repo_name(&path);
    let repo = open_with_retry(&path, &name).await;

    let multi = std::env::var("SHAMIR_RECEIPT_CRASH_MULTI").is_ok_and(|v| v == "1");
    let tombstone = std::env::var("SHAMIR_RECEIPT_CRASH_TOMBSTONE").is_ok_and(|v| v == "1");
    let scenario = Scenario { multi, tombstone };
    let events = scenario.events();

    let hook = Arc::new(test_seam::Hook::default());
    *hook.abort_at.lock().unwrap() = Some(phase);
    test_seam::install(&name, Arc::clone(&hook));

    let result = apply_replicated_batch(&repo, &events, 0, ApplyPolicy::Trusted).await;
    unreachable!("abort must preempt the apply; it returned {result:?}");
}

// ------------------------------------------------------------------ parent

/// Serializes the parent tests: they share the process-global
/// `disable_receipt_lookup` seam and spawn children against shared infra.
static PARENT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Spawns the child scenario in this test binary and returns
/// `(exit status, combined output)`. The child is expected to DIE via
/// `abort()` (non-zero exit: SIGABRT on Unix, exit code 3 on Windows).
fn spawn_child(path: &Path, extra_env: &[(&str, &str)]) -> (std::process::ExitStatus, String) {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .arg(CHILD_TEST)
        .arg("--exact")
        .arg("--test-threads=1")
        .arg("--nocapture")
        .env("SHAMIR_RECEIPT_CRASH_SCENARIO", "receipt-crash")
        .env("SHAMIR_RECEIPT_CRASH_PATH", path);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status, text)
}

fn assert_child_died(status: std::process::ExitStatus, output: &str, point: &str) {
    assert!(
        !status.success(),
        "child at crash point {point} must die via abort(), got success"
    );
    assert!(
        !output.contains("test result: ok"),
        "child at crash point {point} must not report a passing run; output: {output}"
    );
}

/// For one crash point: spawn the aborting child, then reopen TWICE, replaying
/// the SAME batch after each open, and verify full convergence each time
/// (identical allocation, byte-identical journal, counters, retired receipt).
async fn run_point(scenario: Scenario) {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name(temp.path());

    let mut env = vec![];
    if scenario.multi {
        env.push(("SHAMIR_RECEIPT_CRASH_MULTI", "1"));
    }
    if scenario.tombstone {
        env.push(("SHAMIR_RECEIPT_CRASH_TOMBSTONE", "1"));
    }
    let (status, output) = {
        let mut env = env.clone();
        env.push(("SHAMIR_RECEIPT_CRASH_POINT", scenario.point));
        spawn_child(temp.path(), &env)
    };
    assert_child_died(status, &output, scenario.point);

    let events = scenario.events();

    // First reopen + replay (resumes / converges the crashed receipt).
    let journal_snapshot;
    let history_snapshot;
    {
        let repo = open_with_retry(temp.path(), &name).await;
        let outcome = apply_replicated_batch(&repo, &events, 0, ApplyPolicy::Trusted).await;
        assert!(outcome.is_ok(), "first replay failed: {outcome:?}");
        assert_converged(&repo, &scenario).await;
        journal_snapshot = collect_journals(&repo, scenario.leader_watermark()).await;
        history_snapshot = history_rows(&repo, &scenario.keys()).await;
        close(repo).await;
    }

    // Second reopen + replay: the batch is already applied; the invariants
    // must hold UNCHANGED (no new versions, no journal divergence).
    {
        let repo = open_with_retry(temp.path(), &name).await;
        let outcome = apply_replicated_batch(&repo, &events, 0, ApplyPolicy::Trusted).await;
        assert!(
            matches!(outcome, Ok(_)),
            "second replay failed: {outcome:?}"
        );
        assert_converged(&repo, &scenario).await;
        assert_eq!(
            collect_journals(&repo, scenario.leader_watermark()).await,
            journal_snapshot,
            "journal bytes must be identical across the two replays"
        );
        assert_eq!(
            history_rows(&repo, &scenario.keys()).await,
            history_snapshot,
            "history allocation must be identical across the two replays"
        );
        close(repo).await;
    }
    // `temp` drops here: every temp dir is cleaned.
}

async fn collect_journals(repo: &RepoInstance, watermark: u64) -> Vec<Option<Bytes>> {
    let mut rows = Vec::new();
    for version in 1..=watermark {
        rows.push(journal_bytes(repo, version).await);
    }
    rows
}

#[tokio::test]
async fn crash_reopen_replay_convergence_matrix() {
    let _guard = PARENT_LOCK.lock().await;
    for point in ["prepared", "materialized", "checkpointed"] {
        run_point(Scenario {
            point,
            multi: false,
            tombstone: false,
        })
        .await;
    }
    // MULTI (repeated key → absolute counter) and TOMBSTONE (delete row)
    // variants: one crash point each keeps the total runtime modest.
    run_point(Scenario {
        multi: true,
        tombstone: false,
    })
    .await;
    run_point(Scenario {
        multi: false,
        tombstone: true,
    })
    .await;
}

/// NEGATIVE CONTROL: crash through the receipt pipeline at "materialized",
/// then replay with the receipt-lookup seam DISABLED — the legacy (unprotected)
/// path must provably violate the identical-allocation invariant: it allocates
/// a REPLACEMENT version on top of the receipt's version instead of re-owning
/// it, and then fail-stops on the v3 bookmark it cannot downgrade.
#[tokio::test]
async fn negative_control_lookup_disabled_creates_extra_versions() {
    let _guard = PARENT_LOCK.lock().await;
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name(temp.path());
    let (status, output) = spawn_child(
        temp.path(),
        &[
            ("SHAMIR_RECEIPT_CRASH_POINT", "materialized"),
            // Documented for harness self-description; see the child's note:
            // the crash itself must go through the receipt pipeline, the
            // parent honors this flag for its in-process replay instead.
            ("SHAMIR_RECEIPT_CRASH_LOOKUP_DISABLED", "1"),
        ],
    );
    assert_child_died(status, &output, "materialized");

    let repo = open_with_retry(temp.path(), &name).await;
    let key = RecordId([1; 16]).to_bytes().to_vec();

    // Reasoning (verified against the code): with lookup disabled the route
    // is legacy; the crashed receipt left a v3 bookmark record, so legacy
    // `finish_batch` writes the batch at a NEW allocation first and only
    // then fails in `persist_batch_record` → `save_bookmark_and_clean_horizon`
    // (`refuse_v3`) with `RepoRequiresReopen`.
    disable_receipt_lookup(true);
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            disable_receipt_lookup(false);
        }
    }
    let _reset = Reset;
    let error = apply_replicated_batch(&repo, &[event(1, 1)], 0, ApplyPolicy::Trusted).await;
    drop(_reset);

    // (a) the replay ERRORS on the v3 record it cannot persist over...
    assert!(
        matches!(
            &error,
            Err(shamir_storage::error::DbError::RepoRequiresReopen(_))
        ),
        "legacy replay must fail-stop on the outstanding v3 bookmark, got {error:?}"
    );
    // ...but only AFTER writing history rows at the NEW (replacement)
    // allocation: the receipt's version 1 stays, and an extra version 2
    // appears — the exactly-one-version invariant of the receipt path is
    // violated.
    let rows = history_rows(&repo, &[key]).await;
    assert_eq!(
        rows.iter().filter(|(_, version, _)| *version == 1).count(),
        1,
        "the crashed receipt's version 1 remains"
    );
    assert_eq!(
        rows.iter().filter(|(_, version, _)| *version == 2).count(),
        1,
        "legacy replay allocated a REPLACEMENT version 2: unprotected divergence"
    );
    let journal_two = journal_bytes(&repo, 2).await.unwrap();
    assert!(!journal_two.is_empty(), "replacement journal row exists");

    close(repo).await;
}
