//! R5 PR3 shell state/data tests: the configured-subscription golden over
//! a real builder-seeded subscription. Drives the REAL production shell
//! through the shared helpers of [`super::state_data_common`].

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use tokio::time::timeout;
use zeroize::Zeroizing;

use super::harness::{
    overwrite_profile, run_shell, seed_catalogue, ShellFixture, DB_NAME, REPO_NAME,
};
use super::secure_fixture::boot_secure;
use super::state_data_common::{data, leaf_u64, scram_profile, BOUND};

// ── Configured-subscription golden ────────────────────────────────────

/// Seed a REAL configured subscription over the builder-only wire: a
/// publication, a Pull/ReadOnly replication profile scoped to the fixture
/// repository, and a PAUSED subscription bound to it (no live peer is
/// dialed; the upstream is a well-formed `tcp://` URL at a bound-then-
/// closed port, the same shape the existing paused fixtures seed).
/// Returns after the batch is acknowledged.
fn seed_subscription(fixture: &ShellFixture) {
    fixture.guard.mark("seed: configured subscription");
    let addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    fixture.rt.run(async move {
        let client = timeout(
            BOUND,
            super::harness::connect_tls_owned(addr, pin, password),
        )
        .await
        .expect("subscription seed connect hang bound")
        .expect("subscription seed verified TLS connect");
        let scope = ddl::repl_scope(DB_NAME).repo(REPO_NAME).build();
        let mut batch = Batch::new();
        batch.id("state_data_subscription_seed");
        batch.op(
            "publication",
            ddl::publication("state_data_pub")
                .scope(scope.clone())
                .build(),
        );
        batch.op(
            "profile",
            ddl::replication_profile("state_data_prof").stream(
                scope,
                ReplDirection::Pull,
                ReplMode::ReadOnly,
            ),
        );
        batch.op(
            "sub",
            ddl::paused_subscription(
                "state_data_sub",
                "tcp://127.0.0.1:1",
                "state_data_pub",
                "state_data_prof",
            ),
        );
        let response = timeout(BOUND, client.execute(DB_NAME, batch.build()))
            .await
            .expect("subscription seed hang bound")
            .expect("subscription seed batch executes");
        for alias in ["publication", "profile", "sub"] {
            assert!(
                !response.results.contains_key("__error"),
                "subscription seed batch failed: __error present"
            );
            let result = response
                .results
                .get(alias)
                .unwrap_or_else(|| panic!("subscription seed lost alias {alias}"));
            assert!(
                !result.skipped,
                "subscription seed op `{alias}` was skipped"
            );
        }
        client.close().await;
    })
}

/// `replication subscriptions` against a CONFIGURED subscription (the
/// empty-fixture golden above is not enough): the projection carries
/// EXACTLY the allowlisted fields of the engine's subscription record
/// (`execute/replication_seed/status.rs`) under the root
/// `{ subscriptions: [...] }` shape — identities, state, installation
/// state, bookmark/pull versions, the consistent version and the
/// certified gap endpoints — and NEVER the dropped hostile fields
/// (`status_message`, `last_terminal_reason`, source paths,
/// `schema_digest`, `subscription_state`). No secret material and no
/// upstream address appear in the output.
#[test]
fn replication_subscriptions_configured_golden() {
    let secure = boot_secure("replication_subscriptions_configured_golden");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_subscription(fixture);
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
    let entries = payload["subscriptions"]
        .as_array()
        .expect("subscriptions array");
    assert_eq!(entries.len(), 1, "exactly the configured subscription");
    let entry = &entries[0];
    // The configured identities, exactly as bound above.
    assert_eq!(entry["name"].as_str(), Some("state_data_sub"));
    assert_eq!(entry["db"].as_str(), Some(DB_NAME));
    assert_eq!(entry["repo"].as_str(), Some(REPO_NAME));
    assert_eq!(
        entry["state"].as_str(),
        Some("paused"),
        "a paused subscription must report its configured state"
    );
    assert_eq!(
        entry["installation_state"].as_str(),
        Some("ordinary"),
        "a subscription without a seed install reports the ordinary state"
    );
    // Version fields render as decimal strings; the certified gap
    // endpoints are NULL on an ORDINARY repo (no seed receipt, no
    // certified interval) — null OR a decimal string is accepted, but the
    // bookmark/pull/consistent versions are always present on a bound
    // repo. Assert each class exactly.
    for field in [
        "upstream_bookmark",
        "next_pull_version",
        "local_consistent_version",
    ] {
        assert!(
            leaf_u64(&entry[field]).is_some(),
            "subscription field `{field}` must render as a decimal string"
        );
    }
    for field in ["certified_gap_first", "certified_gap_through"] {
        assert!(
            entry[field].is_null() || leaf_u64(&entry[field]).is_some(),
            "gap endpoint `{field}` must be null (ordinary repo) or a decimal string"
        );
    }
    // The allowlist: dropped engine fields never appear, projected or not.
    let text = serde_json::to_string(entry).expect("entry serializes");
    for dropped in [
        "status_message",
        "last_terminal_reason",
        "source_db",
        "source_repo",
        "schema_digest",
        "subscription_state",
        "tcp://127.0.0.1:1",
    ] {
        assert!(
            !text.contains(dropped),
            "projected subscription leaked the non-allowlisted field `{dropped}`"
        );
    }
    // The entry carries ONLY allowlisted keys (no peer extras survive).
    if let Some(fields) = entry.as_object() {
        for key in fields.keys() {
            assert!(
                [
                    "db",
                    "repo",
                    "name",
                    "state",
                    "installation_state",
                    "recorded_identity",
                    "observed_identity",
                    "upstream_bookmark",
                    "next_pull_version",
                    "local_consistent_version",
                    "certified_gap_first",
                    "certified_gap_through",
                    "artifact_digest",
                    "source_lineage_id",
                    "source_history_epoch",
                    "seed_source_version",
                    "local_baseline_version",
                ]
                .contains(&key.as_str()),
                "non-allowlisted subscription key `{key}` was projected"
            );
        }
    }
}
