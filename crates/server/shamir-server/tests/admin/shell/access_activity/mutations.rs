//! R5 PR4 shell rights-mutation tests: `chmod`, `chown` and `chgrp` over the
//! REAL production shell, including the local confirmations/refusals and the
//! server-side authorization decision. Drives the shared helpers of
//! [`super::state_data_common`].

use serde_json::Value;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::hmac as canon;
use tokio::time::timeout;
use zeroize::Zeroizing;

use super::harness::{
    connect_tls_owned, overwrite_profile, run_shell, seed_catalogue, seed_shell_user, ShellFixture,
    DB_NAME, PLAIN_TABLE, REPO_NAME, SHELL_USER,
};
use super::secure_fixture::{boot_secure, run_with_secret};
use super::state_data_common::{data, leaf_u64, refusal, scram_profile, BOUND};

/// Routing database every admin batch rides on (same value as the harness
/// seeding batches).
const ROUTING_DB: &str = "default";

/// The group created by the happy-path test.
const GROUP_NAME: &str = "shelldevs";

// ── Local helpers ────────────────────────────────────────────────────

/// The table address every mutation test targets.
fn table_path() -> String {
    format!("{DB_NAME}/{REPO_NAME}/{PLAIN_TABLE}")
}

/// Point the shell's profile at the bootstrap admin over TLS-TCP.
fn use_admin_profile(fixture: &ShellFixture) {
    let text = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string());
    overwrite_profile(fixture, &text);
}

/// The `rights show` payload of `path`.
fn rights(fixture: &ShellFixture, path: &str) -> Value {
    data(&run_shell(fixture, &["rights", "show", path]), "rights")
}

/// One mode leaf: a strictly four-digit octal rendering.
fn assert_mode(value: &Value, context: &str) {
    let mode = value
        .as_str()
        .unwrap_or_else(|| panic!("{context} mode is not a string"));
    assert!(
        mode.len() == 4 && mode.bytes().all(|byte| (b'0'..=b'7').contains(&byte)),
        "{context} mode is not four octal digits"
    );
}

/// Exactly the `expected` top-level keys of `value` — no missing, no extra.
fn keys_exact(value: &Value, expected: &[&str], context: &str) {
    let fields = value
        .as_object()
        .unwrap_or_else(|| panic!("{context} is not a JSON object"));
    let mut actual = fields.keys().map(String::as_str).collect::<Vec<_>>();
    actual.sort_unstable();
    let mut wanted = expected.to_vec();
    wanted.sort_unstable();
    assert_eq!(actual, wanted, "{context} top-level field set");
}

/// Create one group over the REAL wire with the typed builder only, echoing
/// its server-minted numeric id. Mirrors the HMAC handshake of
/// `harness::seed_shell_user`: the canonical form excludes the tag, so a
/// placeholder tag yields the same canonical bytes as the real op.
fn create_catalogue_group(fixture: &ShellFixture, name: &str) -> u64 {
    fixture.guard.mark("seed: access group");
    let tls_addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    let name = name.to_owned();
    fixture.rt.run(async move {
        let client = timeout(BOUND, connect_tls_owned(tls_addr, pin, password))
            .await
            .expect("group connect hang bound")
            .expect("group verified TLS connect");
        let placeholder = "0".repeat(64);
        let unsigned = ddl::create_group(name.as_str())
            .hmac(&placeholder)
            .build()
            .expect("typed create-group builder (placeholder tag)");
        let key = canon::derive_session_hmac_key(&client.session_id());
        let canonical = canon::canonical_batch_op(ROUTING_DB, &unsigned)
            .expect("canonical form of the create-group op");
        let tag = canon::compute_tag_hex(&key, &canonical);
        let op = ddl::create_group(name.as_str())
            .hmac(&tag)
            .build()
            .expect("typed create-group builder");
        let mut batch = Batch::new();
        batch.id("mk_access_group");
        batch.create_group("mkgroup", op);
        let response = timeout(BOUND, client.execute(ROUTING_DB, batch.build()))
            .await
            .expect("group create hang bound")
            .expect("group create executes");
        let result = response
            .results
            .get("mkgroup")
            .expect("create-group alias present");
        assert_eq!(
            result.records.len(),
            1,
            "create_group must echo exactly one record"
        );
        let id = result.records[0]
            .get_value_u64("group_id")
            .expect("server-minted `group_id` on create_group");
        client.close().await;
        id
    })
}

// ── Tests ────────────────────────────────────────────────────────────

/// `chmod` without `--yes` is refused BEFORE anything is sent (the follow-up
/// `rights show` still reports the original mode); the same command with
/// `--yes` changes the mode and the NEW `rights show` observes it.
#[test]
fn chmod_without_yes_sends_nothing_then_with_yes_changes_mode() {
    let secure = boot_secure("chmod_without_yes_sends_nothing_then_with_yes_changes_mode");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);
    let path = table_path();

    let before = rights(fixture, &path);
    assert_mode(&before["mode"], "chmod before");
    let original = before["mode"]
        .as_str()
        .expect("mode is a string")
        .to_owned();
    assert_ne!(
        original, "0750",
        "the fixture table must not already carry the target mode"
    );

    fixture.guard.mark("chmod: no --yes");
    let refused = run_shell(fixture, &["chmod", path.as_str(), "0750"]);
    refusal(&refused, "chmod", "confirmation_required", 6);
    let unchanged = rights(fixture, &path);
    assert_eq!(
        unchanged["mode"],
        Value::String(original.clone()),
        "an unconfirmed chmod must not reach the server"
    );

    fixture.guard.mark("chmod: --yes");
    let changed = data(
        &run_shell(fixture, &["chmod", path.as_str(), "0750", "--yes"]),
        "chmod",
    );
    keys_exact(&changed, &["path", "mode"], "chmod");
    assert_eq!(changed["path"], path, "chmod must echo the normalized path");
    assert_eq!(changed["mode"], "0750", "chmod must echo the new mode");

    let after = rights(fixture, &path);
    assert_eq!(
        after["mode"], "0750",
        "the confirmed chmod must be visible in the tree"
    );
}

/// The resource root is never mutated from the shell: `chmod root` is the
/// fixed local refusal, exit 6.
#[test]
fn chmod_root_refused_locally() {
    let secure = boot_secure("chmod_root_refused_locally");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);

    fixture.guard.mark("chmod: root");
    let run = run_shell(fixture, &["chmod", "root", "0750", "--yes"]);
    refusal(&run, "chmod", "root_mutation_refused", 6);
}

/// MODE is validated as a one-to-four digit octal literal at most `0o7777`:
/// a non-octal digit and a five-digit literal are both `invalid_mode`, exit 1.
#[test]
fn chmod_invalid_mode_refused() {
    let secure = boot_secure("chmod_invalid_mode_refused");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);
    let path = table_path();

    fixture.guard.mark("chmod: non-octal digit");
    let run = run_shell(fixture, &["chmod", path.as_str(), "8", "--yes"]);
    refusal(&run, "chmod", "invalid_mode", 1);

    fixture.guard.mark("chmod: five-digit mode");
    let run = run_shell(fixture, &["chmod", path.as_str(), "07777", "--yes"]);
    refusal(&run, "chmod", "invalid_mode", 1);
}

/// The authorization decision belongs to the server: a restricted principal
/// passes the shell's local pre-judgement and the server answers the typed
/// `permission_denied`, exit 4.
#[test]
fn chmod_non_superuser_denied() {
    let secure = boot_secure("chmod_non_superuser_denied");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    let user_password = seed_shell_user(fixture);
    let path = table_path();
    let restricted = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string())
        .replace("username: admin\n", &format!("username: {SHELL_USER}\n"));
    overwrite_profile(fixture, &restricted);

    fixture.guard.mark("chmod: restricted principal");
    let run = run_with_secret(
        fixture,
        user_password.clone(),
        false,
        &["chmod", path.as_str(), "0750", "--yes"],
    );
    assert!(
        !run.contains_canary(user_password.as_str()),
        "the restricted user's secret leaked"
    );
    refusal(&run, "chmod", "permission_denied", 4);
}

/// Owner and group names are resolved against the observed directory: an
/// unknown name on either side is `unknown_principal`, exit 5.
#[test]
fn chown_unknown_owner_and_chgrp_unknown_group() {
    let secure = boot_secure("chown_unknown_owner_and_chgrp_unknown_group");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);
    let path = table_path();

    fixture.guard.mark("chown: unknown owner");
    let run = run_shell(fixture, &["chown", path.as_str(), "nosuchuser", "--yes"]);
    refusal(&run, "chown", "unknown_principal", 5);

    fixture.guard.mark("chgrp: unknown group");
    let run = run_shell(fixture, &["chgrp", path.as_str(), "nosuchgroup", "--yes"]);
    refusal(&run, "chgrp", "unknown_principal", 5);
}

/// Happy path of both name-based mutations: a group created through the
/// typed builder is resolvable by `chgrp`, and `chown` resolves the seeded
/// shell user to the SAME principal id the directory reports.
#[test]
fn chown_and_chgrp_happy_path() {
    let secure = boot_secure("chown_and_chgrp_happy_path");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);
    let path = table_path();
    let group_id = create_catalogue_group(fixture, GROUP_NAME);

    fixture.guard.mark("chgrp: known group");
    let grouped = data(
        &run_shell(fixture, &["chgrp", path.as_str(), GROUP_NAME, "--yes"]),
        "chgrp",
    );
    keys_exact(&grouped, &["path", "group", "group_id"], "chgrp");
    assert_eq!(grouped["path"], path, "chgrp must echo the normalized path");
    assert_eq!(
        grouped["group"], GROUP_NAME,
        "chgrp must echo the group name"
    );
    assert_eq!(
        leaf_u64(&grouped["group_id"]),
        Some(group_id),
        "chgrp must resolve the server-minted group id"
    );

    fixture.guard.mark("chown: known owner");
    let owned = data(
        &run_shell(fixture, &["chown", path.as_str(), SHELL_USER, "--yes"]),
        "chown",
    );
    keys_exact(&owned, &["path", "owner", "owner_id"], "chown");
    assert_eq!(owned["path"], path, "chown must echo the normalized path");
    assert_eq!(owned["owner"], SHELL_USER, "chown must echo the owner name");
    let directory = data(&run_shell(fixture, &["user", "list"]), "user");
    let principal = directory["users"]
        .as_array()
        .expect("users array")
        .iter()
        .find(|entry| entry["name"] == SHELL_USER)
        .expect("the seeded shell user appears in the directory");
    assert_eq!(
        owned["owner_id"], principal["principal64"],
        "chown must report the directory's principal id for the owner"
    );
    assert!(
        leaf_u64(&owned["owner_id"]).is_some(),
        "chown `owner_id` is not a decimal string"
    );

    let after = rights(fixture, &path);
    assert_eq!(after["owner_name"], SHELL_USER, "the new owner is observed");
    assert_eq!(
        leaf_u64(&after["group"]),
        Some(group_id),
        "the new group is observed"
    );
}
