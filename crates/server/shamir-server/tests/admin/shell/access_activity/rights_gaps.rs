//! R5 PR4 shell rights gaps: the `group tree --db` narrowing over TWO real
//! databases, the server-side denial of every rights view for a restricted
//! SCRAM principal, the `group members` roster, and the local refusals that
//! must never dial. Drives the REAL production shell through the shared
//! helpers of [`super::state_data_common`].

use serde_json::Value;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::GroupRef;
use shamir_query_types::hmac as canon;
use tokio::time::timeout;
use zeroize::Zeroizing;

use super::harness::{
    connect_tls_owned, overwrite_profile, run_shell, seed_catalogue, seed_shell_user, ShellFixture,
    DB_NAME, SHELL_USER,
};
use super::secure_fixture::{boot_secure, run_with_secret};
use super::state_data_common::{data, leaf_u64, refusal, scram_profile, Relay, BOUND};

/// Routing database every admin batch rides on (same value as the harness
/// seeding batches).
const ROUTING_DB: &str = "default";

/// The SECOND database seeded over the wire, so the `--db` narrowing of
/// `group tree` has something real to exclude.
const SECOND_DB: &str = "shelldb2";

/// The group created by the roster test.
const GROUP_NAME: &str = "shelldevs";

// ── Local helpers ────────────────────────────────────────────────────

/// Point the shell's profile at the bootstrap admin over TLS-TCP.
fn use_admin_profile(fixture: &ShellFixture) {
    let text = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string());
    overwrite_profile(fixture, &text);
}

/// The `database` children of a `group tree` payload, sorted by name.
fn database_names(tree: &Value) -> Vec<String> {
    let mut names = tree["resources"]["children"]
        .as_array()
        .expect("group tree resources.children array")
        .iter()
        .filter(|child| child["kind"] == "database")
        .map(|child| {
            child["name"]
                .as_str()
                .unwrap_or_else(|| panic!("database node name is not a string"))
                .to_owned()
        })
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

/// Create one database over the REAL wire with the typed builder only,
/// asserting the server's `db` echo.
fn create_database(fixture: &ShellFixture, name: &str) {
    fixture.guard.mark("seed: second database");
    let tls_addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    let name = name.to_owned();
    fixture.rt.run(async move {
        let client = timeout(BOUND, connect_tls_owned(tls_addr, pin, password))
            .await
            .expect("second-db connect hang bound")
            .expect("second-db verified TLS connect");
        let mut batch = Batch::new();
        batch.id("mkdb2");
        batch.create_db("mkdb2", ddl::create_db(name.as_str()));
        let response = timeout(BOUND, client.execute(ROUTING_DB, batch.build()))
            .await
            .expect("second-db create hang bound")
            .expect("second-db create executes");
        let result = response
            .results
            .get("mkdb2")
            .expect("create-db alias present");
        assert_eq!(
            result.records.len(),
            1,
            "create_db must echo exactly one record"
        );
        assert_eq!(
            result.records[0].get_value_str("db"),
            Some(name.as_str()),
            "create_db must echo the created database name"
        );
        timeout(BOUND, async move { client.close().await })
            .await
            .expect("client close hang bound");
    })
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
        timeout(BOUND, async move { client.close().await })
            .await
            .expect("client close hang bound");
        id
    })
}

/// Add one principal to one group over the REAL wire with the typed builder
/// only (same canonical-then-sign handshake as the seeds above), asserting
/// the server's echo of both ids.
fn add_catalogue_member(fixture: &ShellFixture, group_id: u64, user: u64) {
    fixture.guard.mark("seed: access group member");
    let tls_addr = fixture.tls_addr;
    let pin = fixture.pin;
    let password = Zeroizing::new(fixture.canary.as_bytes().to_vec());
    fixture.rt.run(async move {
        let client = timeout(BOUND, connect_tls_owned(tls_addr, pin, password))
            .await
            .expect("member connect hang bound")
            .expect("member verified TLS connect");
        let group = GroupRef::Id { id: group_id };
        let placeholder = "0".repeat(64);
        let unsigned = ddl::add_group_member(group.clone(), user)
            .hmac(&placeholder)
            .build()
            .expect("typed add-group-member builder (placeholder tag)");
        let key = canon::derive_session_hmac_key(&client.session_id());
        let canonical = canon::canonical_batch_op(ROUTING_DB, &unsigned)
            .expect("canonical form of the add-group-member op");
        let tag = canon::compute_tag_hex(&key, &canonical);
        let op = ddl::add_group_member(group, user)
            .hmac(&tag)
            .build()
            .expect("typed add-group-member builder");
        let mut batch = Batch::new();
        batch.id("mk_access_group_member");
        batch.add_group_member("addmember", op);
        let response = timeout(BOUND, client.execute(ROUTING_DB, batch.build()))
            .await
            .expect("member add hang bound")
            .expect("member add executes");
        let result = response
            .results
            .get("addmember")
            .expect("add-group-member alias present");
        assert_eq!(
            result.records.len(),
            1,
            "add_group_member must echo exactly one record"
        );
        let record = &result.records[0];
        assert_eq!(
            record.get_value_u64("added_to_group"),
            Some(group_id),
            "add_group_member must echo the target group id"
        );
        assert_eq!(
            record.get_value_u64("user"),
            Some(user),
            "add_group_member must echo the added principal id"
        );
        timeout(BOUND, async move { client.close().await })
            .await
            .expect("client close hang bound");
    })
}

// ── Tests ────────────────────────────────────────────────────────────

/// With TWO real databases the UNFILTERED tree lists both, while
/// `group tree --db <DB_NAME>` narrows the database set to EXACTLY the
/// requested one. Mutant: dropping the `.db(...)` from the tree request
/// leaves both databases in the filtered answer while the shell still
/// echoes `database_filter`, so the SET assertion fails.
#[test]
fn group_tree_db_filter_narrows_the_database_set() {
    let secure = boot_secure("group_tree_db_filter_narrows_the_database_set");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);
    create_database(fixture, SECOND_DB);

    fixture.guard.mark("rights: unfiltered tree");
    let unfiltered = data(&run_shell(fixture, &["group", "tree"]), "group");
    assert!(
        unfiltered["database_filter"].is_null(),
        "an unfiltered tree must report a null database_filter"
    );
    let all = database_names(&unfiltered);
    assert!(
        all.contains(&DB_NAME.to_string()),
        "the unfiltered tree must list the seeded database"
    );
    assert!(
        all.contains(&SECOND_DB.to_string()),
        "the unfiltered tree must list the second database"
    );

    fixture.guard.mark("rights: filtered tree");
    let filtered = data(
        &run_shell(fixture, &["group", "tree", "--db", DB_NAME]),
        "group",
    );
    assert_eq!(
        filtered["database_filter"],
        Value::String(DB_NAME.to_string()),
        "the filtered tree must echo the requested database"
    );
    assert_eq!(
        database_names(&filtered),
        vec![DB_NAME.to_string()],
        "the --db filter must narrow the database SET to exactly the requested one"
    );
}

/// Every rights view is the server's decision: the restricted SCRAM
/// principal is refused with `permission_denied`/exit 4 on ALL of them, and
/// the SAME fixture/server still answers the admin's `user list`, so the
/// denial is not a local pre-judgement.
#[test]
fn ordinary_user_is_denied_every_rights_view() {
    let secure = boot_secure("ordinary_user_is_denied_every_rights_view");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    let user_password = seed_shell_user(fixture);
    let restricted = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string())
        .replace("username: admin\n", &format!("username: {SHELL_USER}\n"));
    overwrite_profile(fixture, &restricted);

    for (args, command_part) in [
        (&["user", "list"][..], "user"),
        (&["group", "tree"][..], "group"),
        (&["group", "members"][..], "group"),
        (&["rights", "show", DB_NAME][..], "rights"),
    ] {
        fixture
            .guard
            .mark(format!("restricted: {}", args.join(" ")));
        let run = run_with_secret(fixture, user_password.clone(), false, args);
        assert!(
            !run.contains_canary(user_password.as_str()),
            "the restricted user's secret leaked"
        );
        refusal(&run, command_part, "permission_denied", 4);
    }

    use_admin_profile(fixture);
    let directory = data(&run_shell(fixture, &["user", "list"]), "user");
    assert!(
        directory["users"]
            .as_array()
            .is_some_and(|users| users.iter().any(|entry| entry["name"] == SHELL_USER)),
        "the admin must still read the principal directory of the same server"
    );
}

/// `group members` reports the REAL roster: a group created through the
/// typed builder, with the seeded shell user added through the typed
/// member builder, appears by name with the SAME `principal64` id the
/// principal directory reports. Mutant: a projection that drops members, or
/// a request that omits them.
#[test]
fn group_members_reports_the_seeded_roster() {
    let secure = boot_secure("group_members_reports_the_seeded_roster");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);

    let directory = data(&run_shell(fixture, &["user", "list"]), "user");
    let principal = directory["users"]
        .as_array()
        .expect("users array")
        .iter()
        .find(|entry| entry["name"] == SHELL_USER)
        .expect("the seeded shell user appears in the directory");
    let principal_id = leaf_u64(&principal["principal64"])
        .expect("the directory `principal64` is a decimal string");
    let group_id = create_catalogue_group(fixture, GROUP_NAME);
    add_catalogue_member(fixture, group_id, principal_id);

    fixture.guard.mark("rights: group members roster");
    let members = data(&run_shell(fixture, &["group", "members"]), "group");
    let group = members["groups"]
        .as_array()
        .expect("groups array")
        .iter()
        .find(|group| group["name"] == GROUP_NAME)
        .expect("the created group appears in `group members`");
    assert_eq!(
        leaf_u64(&group["id"]),
        Some(group_id),
        "the roster entry must carry the server-minted group id"
    );
    let roster = group["members"]
        .as_array()
        .expect("the group entry `members` roster array");
    assert!(
        roster
            .iter()
            .any(|member| leaf_u64(&member["id"]) == Some(principal_id)),
        "the roster must contain the seeded shell user's principal64 id"
    );
}

/// The resource root and malformed selectors are refused LOCALLY: the
/// counting relay in front of the TLS listener observes zero downstream
/// accepts for `chmod root`, an over-long `rights show` path and a store
/// component the engine's name rule rejects. Mutant: a local refusal moved
/// after `with_session`.
#[test]
fn root_and_malformed_selectors_send_no_request() {
    let secure = boot_secure("root_and_malformed_selectors_send_no_request");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    let relay = Relay::start(fixture, fixture.tls_addr);
    let text = scram_profile(fixture, "tls-tcp", &relay.addr.to_string());
    overwrite_profile(fixture, &text);

    fixture.guard.mark("local: chmod root");
    let run = run_shell(fixture, &["chmod", "root", "0750", "--yes"]);
    refusal(&run, "chmod", "root_mutation_refused", 6);

    fixture.guard.mark("local: over-long selector");
    let run = run_shell(fixture, &["rights", "show", "aa/bb/cc/dd"]);
    refusal(&run, "rights", "usage", 1);

    // A database component with a rejected byte cannot be used as argv, so
    // the STORE component carries it: `valid_name_component` refuses it
    // before any request.
    fixture.guard.mark("local: rejected store component");
    let run = run_shell(fixture, &["chmod", "okname/bad:store", "0750", "--yes"]);
    refusal(&run, "chmod", "usage", 1);

    assert_eq!(
        relay.finish(fixture),
        0,
        "a locally refused selector must not open a downstream connection"
    );
}
