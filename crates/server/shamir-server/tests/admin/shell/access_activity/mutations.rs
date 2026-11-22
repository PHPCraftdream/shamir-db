//! R5 PR4 shell rights-mutation tests: `chmod`, `chown` and `chgrp` over the
//! REAL production shell, including the local confirmations/refusals and the
//! server-side authorization decision. The signing seam is proven over the
//! real wire (a `sign()`ed op is accepted, a wrong tag and an unknown target
//! are refused by the server itself), and the zero-dial claims (unconfirmed
//! mutations, the §3.3 token refusal, locally invalid selectors) use the
//! counting relay. Drives the shared helpers of [`super::state_data_common`].

use serde_json::Value;
use shamir_client::Client;
use shamir_client::ClientError as Wire;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::token_admin;
use shamir_query_types::batch::BatchOp;
use shamir_query_types::hmac as canon;
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use tokio::time::timeout;
use zeroize::Zeroizing;

use super::harness::{
    connect_tls_owned, overwrite_profile, run_shell, seed_catalogue, seed_shell_user, ShellFixture,
    DB_NAME, PLAIN_TABLE, REPO_NAME, SHELL_USER,
};
use super::secure_fixture::{
    boot_secure, provision_token, run_token_shell, run_with_secret, write_secure_profile,
};
use super::state_data_common::{
    data, leaf_u64, refusal, scram_profile, Relay, BOUND, TOKEN_SUBSET,
};

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

/// The signing seam over the REAL wire. A `sign()`ed chmod is accepted by the
/// server and observed by a following `rights show`; a WRONG tag and an
/// UNKNOWN target are refused by the server itself with its own typed code,
/// and neither changes the mode.
///
/// Mutants caught: a client that sends an unsigned/placeholder tag (the
/// wrong-tag send would then succeed), a shell that fabricates the effect
/// without the server, a mapping that flattened every refusal into one code.
#[test]
fn signed_chmod_over_the_wire_is_accepted_and_a_wrong_tag_is_refused() {
    let secure = boot_secure("signed_chmod_over_the_wire_is_accepted_and_a_wrong_tag_is_refused");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);
    let path = table_path();
    let before = rights(fixture, &path);
    assert_mode(&before["mode"], "wire chmod before");

    fixture.guard.mark("wire: signed chmod probes");
    let tls_addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    // One live SCRAM connection; the session key is derived from it exactly
    // as the production client does.
    let (accepted_mode, wrong_code, unknown_code) = fixture.rt.run(async move {
        let client = timeout(BOUND, connect_tls_owned(tls_addr, pin, password))
            .await
            .expect("wire mutation connect hang bound")
            .expect("wire mutation verified TLS connect");
        let key = canon::derive_session_hmac_key(&client.session_id());
        let resource = ddl::res::table(DB_NAME, REPO_NAME, PLAIN_TABLE);

        // 1. correctly signed: accepted.
        let signed = ddl::chmod(resource.clone(), 0o705)
            .sign(&key, ROUTING_DB)
            .expect("signed chmod op");
        let mut batch = Batch::new();
        batch.id("wire_chmod_signed");
        batch.op("chmod", signed);
        let response = timeout(BOUND, client.execute(ROUTING_DB, batch.build()))
            .await
            .expect("signed chmod hang bound")
            .expect("the server must accept a correctly signed chmod");
        let result = response
            .results
            .get("chmod")
            .expect("signed chmod alias present");
        assert_eq!(
            result.records.len(),
            1,
            "an accepted chmod must echo exactly one record"
        );
        assert_eq!(
            result.records[0].get_value_i64("mode"),
            Some(i64::from(0o705)),
            "the server must echo the applied mode"
        );

        // 2. deliberately wrong tag: refused with `hmac_mismatch`.
        let wrong = ddl::chmod(resource, 0o707)
            .hmac("0".repeat(64))
            .build()
            .expect("typed chmod with a placeholder tag");
        let mut batch = Batch::new();
        batch.id("wire_chmod_wrong_tag");
        batch.op("chmod", wrong);
        let wrong_code = execute_code(&client, batch.build()).await;

        // 3. unknown target, correctly signed: the server's own refusal.
        let unknown_resource = ddl::res::table("nosuchdb", "nosuchrepo", "nosuchtable");
        let unknown = ddl::chmod(unknown_resource, 0o705)
            .sign(&key, ROUTING_DB)
            .expect("signed chmod op for an unknown target");
        let mut batch = Batch::new();
        batch.id("wire_chmod_unknown_target");
        batch.op("chmod", unknown);
        let unknown_code = execute_code(&client, batch.build()).await;

        client.close().await;
        (0o705u16, wrong_code, unknown_code)
    });

    assert_eq!(
        wrong_code, "hmac_mismatch",
        "a placeholder tag must be refused by the server's HMAC gate"
    );
    // The unknown target fails while the mode is applied: the engine's
    // `set_resource_meta` reports an alias-less `NotFound` QueryError, which
    // the server classifies as `unknown_db` (its OWN typed code, distinct
    // from both `hmac_mismatch` and `access_denied`).
    assert_eq!(
        unknown_code, "unknown_db",
        "an unknown target must be refused by the server, not by the client"
    );

    let after = rights(fixture, &path);
    assert_eq!(
        after["mode"],
        format!("{accepted_mode:04o}"),
        "only the signed change applied"
    );
}

/// The signing seam for the two NAME-based mutations over the REAL wire: a
/// `sign()`ed `chown` and `chgrp` are accepted, a MISSING tag is refused by
/// the server's own coarse gate (`hmac_required`), a WRONG tag by its tag
/// verification (`hmac_mismatch`), and a correctly signed request for an
/// UNKNOWN target by the target lookup (`unknown_db`) — and none of the
/// three refusals changes the resource.
///
/// The operations are built by the TYPED builder and only the optional tag
/// field is cleared for the missing-tag case (the shape
/// `tests/admin/identity/permission_e2e/common.rs` uses), so this never
/// hand-assembles a wire value.
///
/// Mutants caught: a client that strips or reuses a tag (the missing-tag
/// send would then be accepted), a server path that skips tag verification
/// for chown/chgrp, and a refusal that still applies the effect.
#[test]
fn signed_chown_and_chgrp_are_required_and_unknown_targets_are_refused() {
    let secure = boot_secure("signed_chown_and_chgrp_are_required_and_unknown_targets_are_refused");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);
    let path = table_path();
    let group_id = create_catalogue_group(fixture, GROUP_NAME);
    let directory = data(&run_shell(fixture, &["user", "list"]), "user");
    let owner_id = directory["users"]
        .as_array()
        .expect("users array")
        .iter()
        .find(|entry| entry["name"] == SHELL_USER)
        .and_then(|entry| leaf_u64(&entry["principal64"]))
        .expect("the seeded shell user's principal id");
    // The refused probes target DIFFERENT values than the accepted ones (another
    // principal, no group), so an erroneously applied refused request changes
    // what the final `rights show` reports instead of hiding behind a no-op.
    let other_owner_id = directory["users"]
        .as_array()
        .expect("users array")
        .iter()
        .filter_map(|entry| leaf_u64(&entry["principal64"]))
        .find(|id| *id != owner_id)
        .expect("a second principal to probe with");

    fixture.guard.mark("wire: chown/chgrp tag probes");
    let tls_addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    let codes = fixture.rt.run(async move {
        let client = timeout(BOUND, connect_tls_owned(tls_addr, pin, password))
            .await
            .expect("tag-probe connect hang bound")
            .expect("tag-probe verified TLS connect");
        let key = canon::derive_session_hmac_key(&client.session_id());
        let resource = ddl::res::table(DB_NAME, REPO_NAME, PLAIN_TABLE);

        // 1. correctly signed chown: accepted.
        let signed_chown = ddl::chown(resource.clone(), owner_id)
            .sign(&key, ROUTING_DB)
            .expect("signed chown op");
        let mut batch = Batch::new();
        batch.id("wire_chown_signed");
        batch.op("chown", signed_chown);
        timeout(BOUND, client.execute(ROUTING_DB, batch.build()))
            .await
            .expect("signed chown hang bound")
            .expect("the server must accept a correctly signed chown");

        // 2. chown with the tag stripped: the coarse gate refuses it. The
        // builder refuses to build without a tag, so a placeholder is attached
        // and then cleared (the shape `permission_e2e/common.rs` uses).
        let mut batch = Batch::new();
        batch.id("wire_chown_no_tag");
        batch.op(
            "chown",
            without_tag(
                ddl::chown(resource.clone(), other_owner_id)
                    .hmac("unused")
                    .build()
                    .expect("typed chown with a placeholder tag"),
            ),
        );
        let chown_missing = execute_code(&client, batch.build()).await;

        // 3. chown with a wrong tag: tag verification refuses it.
        let mut batch = Batch::new();
        batch.id("wire_chown_wrong_tag");
        batch.op(
            "chown",
            ddl::chown(resource.clone(), other_owner_id)
                .hmac("0".repeat(64))
                .build()
                .expect("typed chown with a placeholder tag"),
        );
        let chown_wrong = execute_code(&client, batch.build()).await;

        // 4. correctly signed chown for an UNKNOWN target.
        let mut batch = Batch::new();
        batch.id("wire_chown_unknown_target");
        batch.op(
            "chown",
            ddl::chown(
                ddl::res::table("nosuchdb", "nosuchrepo", "nosuchtable"),
                owner_id,
            )
            .sign(&key, ROUTING_DB)
            .expect("signed chown op for an unknown target"),
        );
        let chown_unknown = execute_code(&client, batch.build()).await;

        // 5. correctly signed chgrp: accepted.
        let signed_chgrp = ddl::chgrp(resource.clone(), Some(group_id))
            .sign(&key, ROUTING_DB)
            .expect("signed chgrp op");
        let mut batch = Batch::new();
        batch.id("wire_chgrp_signed");
        batch.op("chgrp", signed_chgrp);
        timeout(BOUND, client.execute(ROUTING_DB, batch.build()))
            .await
            .expect("signed chgrp hang bound")
            .expect("the server must accept a correctly signed chgrp");

        // 6. chgrp with the tag stripped.
        let mut batch = Batch::new();
        batch.id("wire_chgrp_no_tag");
        batch.op(
            "chgrp",
            without_tag(
                ddl::chgrp(resource.clone(), None)
                    .hmac("unused")
                    .build()
                    .expect("typed chgrp with a placeholder tag"),
            ),
        );
        let chgrp_missing = execute_code(&client, batch.build()).await;

        // 7. chgrp with a wrong tag.
        let mut batch = Batch::new();
        batch.id("wire_chgrp_wrong_tag");
        batch.op(
            "chgrp",
            ddl::chgrp(resource, None)
                .hmac("0".repeat(64))
                .build()
                .expect("typed chgrp with a placeholder tag"),
        );
        let chgrp_wrong = execute_code(&client, batch.build()).await;

        // Bounded teardown: `close` awaits a write lock and the transport
        // shutdown, so it carries the same hang bound as every other step.
        timeout(BOUND, async move { client.close().await })
            .await
            .expect("tag-probe close hang bound");
        (
            chown_missing,
            chown_wrong,
            chown_unknown,
            chgrp_missing,
            chgrp_wrong,
        )
    });

    let (chown_missing, chown_wrong, chown_unknown, chgrp_missing, chgrp_wrong) = codes;
    assert_eq!(
        chown_missing, "hmac_required",
        "a chown without a tag must be refused by the server"
    );
    assert_eq!(
        chown_wrong, "hmac_mismatch",
        "a wrongly tagged chown must be refused by the server"
    );
    assert_eq!(
        chown_unknown, "unknown_db",
        "a signed chown for an unknown target must be refused by the server"
    );
    assert_eq!(
        chgrp_missing, "hmac_required",
        "a chgrp without a tag must be refused by the server"
    );
    assert_eq!(
        chgrp_wrong, "hmac_mismatch",
        "a wrongly tagged chgrp must be refused by the server"
    );

    // The two accepted requests applied (owner and group are the signed
    // values) and the refused probes, which asked for a different owner and for
    // no group, changed nothing on top of them.
    let after = rights(fixture, &path);
    assert_eq!(
        after["owner_name"], SHELL_USER,
        "a refused chown must not replace the signed owner"
    );
    assert_eq!(
        leaf_u64(&after["group"]),
        Some(group_id),
        "a refused chgrp must not replace the signed group"
    );
}

/// Clear ONLY the optional tag of a typed signing op, leaving every other
/// field exactly as the builder produced it (no hand-built wire value).
fn without_tag(op: BatchOp) -> BatchOp {
    match op {
        BatchOp::Chown(mut inner) => {
            inner.hmac = None;
            BatchOp::Chown(inner)
        }
        BatchOp::Chgrp(mut inner) => {
            inner.hmac = None;
            BatchOp::Chgrp(inner)
        }
        other => panic!("expected a signing op, got {other:?}"),
    }
}

/// An unconfirmed `chmod`/`chown`/`chgrp` opens NO connection at all: the
/// counting relay in front of the TLS listener observes zero downstream
/// accepts. Mutant: moving the confirmation after `with_session` (the relay
/// sees a connection) or after the request.
#[test]
fn unconfirmed_mutations_open_no_connection() {
    let secure = boot_secure("unconfirmed_mutations_open_no_connection");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    let relay = Relay::start(fixture, fixture.tls_addr);
    let text = scram_profile(fixture, "tls-tcp", &relay.addr.to_string());
    overwrite_profile(fixture, &text);
    let path = table_path();

    for (args, command_part) in [
        (&["chmod", path.as_str(), "0750"][..], "chmod"),
        (&["chown", path.as_str(), SHELL_USER][..], "chown"),
        (&["chgrp", path.as_str(), GROUP_NAME][..], "chgrp"),
    ] {
        fixture.guard.mark(format!("unconfirmed: {command_part}"));
        let run = run_shell(fixture, args);
        let envelope = refusal(&run, command_part, "confirmation_required", 6);
        assert!(
            envelope["data"].is_null(),
            "a confirmation refusal must carry no data leaf"
        );
    }
    assert_eq!(
        relay.finish(fixture),
        0,
        "an unconfirmed mutation must not open a downstream connection"
    );
}

/// §3.3 for the mutation commands: a token session refuses each of them with
/// the fixed subset message and exit 4, and the profile points at an address
/// NOTHING listens on, so any dial would surface as `connection_failed`
/// (exit 2) instead. Mutant: a command that re-entered the token allowlist
/// would exit 2.
///
/// The four read-only rights views are repeated here at the same dead
/// address (their relay-backed coverage lives in
/// `access.rs::token_session_refuses_rights_commands`) so the whole §3.3
/// rights set is pinned against a dial in one place.
#[test]
fn token_session_refuses_mutations_without_dialing() {
    let secure = boot_secure("token_session_refuses_mutations_without_dialing");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    let token = provision_token(
        fixture,
        token_admin::create_api_token("https")
            .for_user(SHELL_USER)
            .databases([DB_NAME])
            .mode(ApiTokenMode::ReadOnly)
            .channels(ApiTokenChannels::Both),
    );
    // The counting relay sits in front of the HTTPS listener: the token
    // gate must refuse all seven commands without accepting a single
    // downstream connection (the relay reports `1` for a real dial — see
    // `state_data::token_subset::token_subset_zero_dial_proven_by_relay_counter`).
    let relay = Relay::start(fixture, secure.http_addr);
    write_secure_profile(fixture, "https", relay.addr, DB_NAME, Some(&secure.ca_path));
    let path = table_path();

    for (args, command_part) in [
        (&["user", "list"][..], "user"),
        (&["group", "tree"][..], "group"),
        (&["group", "members"][..], "group"),
        (&["rights", "show", DB_NAME][..], "rights"),
        (&["chmod", path.as_str(), "0750", "--yes"][..], "chmod"),
        (&["chown", path.as_str(), SHELL_USER, "--yes"][..], "chown"),
        (&["chgrp", path.as_str(), GROUP_NAME, "--yes"][..], "chgrp"),
    ] {
        fixture.guard.mark(format!("token: {}", args.join(" ")));
        let run = run_token_shell(fixture, &token, args);
        let value = refusal(&run, command_part, "token_request_denied", 4);
        assert!(
            value["message"].as_str() == Some(TOKEN_SUBSET),
            "wrong fixed token-subset message for `{command_part}`"
        );
    }
    assert_eq!(
        relay.finish(fixture),
        0,
        "a refused mutation must not open a downstream connection"
    );
}

// ── Wire helpers ─────────────────────────────────────────────────────

/// The server's typed refusal code of one batch that is expected to fail.
/// Reads only the code; the server message is never asserted or printed.
async fn execute_code(client: &Client, request: shamir_query_types::batch::BatchRequest) -> String {
    match timeout(BOUND, client.execute(ROUTING_DB, request))
        .await
        .expect("refusal hang bound")
    {
        Ok(_) => String::from("accepted"),
        Err(error) => client_error_code(&error),
    }
}

/// The stable CODE of one client-side failure: a refusal the server typed
/// keeps the server's own code, every other variant reports its NAME. Neither
/// the server message nor any handshake text is carried onward.
fn client_error_code(error: &shamir_client::ClientError) -> String {
    match error {
        Wire::Db { code, .. } | Wire::Server { code } => code.clone(),
        Wire::Io(_) => String::from("io"),
        Wire::Tls { .. } => String::from("tls"),
        Wire::Transport { .. } => String::from("transport"),
        Wire::Handshake { .. } => String::from("handshake"),
        Wire::Protocol(_) => String::from("protocol"),
        Wire::IdentityRotationPending { .. } => String::from("identity_rotation_pending"),
        Wire::TransactionOutcome(_) => String::from("transaction_outcome"),
        Wire::AsOfVisibilityTimeout { .. } => String::from("as_of_visibility_timeout"),
        Wire::AsOfFutureVersion { .. } => String::from("as_of_future_version"),
        Wire::AsOfReopenRequired { .. } => String::from("as_of_reopen_required"),
        Wire::ResyncRequired { .. } => String::from("resync_required"),
        Wire::VisibilityFutureVersion { .. } => String::from("visibility_future_version"),
        Wire::VisibilityWaitInvalid { .. } => String::from("visibility_wait_invalid"),
        Wire::WriteCommittedVisibilityPending { .. } => {
            String::from("write_committed_visibility_pending")
        }
        Wire::RequestIdMismatch { .. } => String::from("request_id_mismatch"),
        Wire::IdEncodingWithRefs { .. } => String::from("id_encoding_with_refs"),
        Wire::Encode(_) => String::from("encode"),
        Wire::Decode(_) => String::from("decode"),
        Wire::InvalidUsername(_) => String::from("invalid_username"),
        Wire::ConnectionClosed(_) => String::from("connection_closed"),
        Wire::ConnectTimeout(_) => String::from("connect_timeout"),
        Wire::RequestTimeout(_) => String::from("request_timeout"),
        Wire::HandshakeTimeout(..) => String::from("handshake_timeout"),
        Wire::Http { .. } => String::from("http"),
        Wire::InvalidToken => String::from("invalid_token"),
    }
}
