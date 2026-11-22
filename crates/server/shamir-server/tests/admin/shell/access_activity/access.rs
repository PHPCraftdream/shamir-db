//! R5 PR4 shell rights tests: the principal directory, the access tree and
//! the `rights show` selector, plus the SCRAM-only token refusal for the
//! rights commands. Drives the REAL production shell through the shared
//! helpers of [`super::state_data_common`].

use serde_json::Value;
use shamir_query_builder::token_admin;
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};

use super::harness::{
    overwrite_profile, run_shell, seed_catalogue, seed_shell_user, ShellFixture, DB_NAME,
    SHELL_USER,
};
use super::secure_fixture::{
    boot_secure, provision_token, run_token_shell, run_with_secret, write_secure_profile,
};
use super::state_data_common::{data, leaf_u64, refusal, scram_profile, Relay, TOKEN_SUBSET};

// ── Local shape helpers ──────────────────────────────────────────────

/// The JSON object of `value`, with a call-site label in the panic.
fn object<'a>(value: &'a Value, context: &str) -> &'a serde_json::Map<String, Value> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{context} is not a JSON object"))
}

/// Exactly the `expected` top-level keys of `value` — no missing, no extra.
fn keys_exact(value: &Value, expected: &[&str], context: &str) {
    let fields = object(value, context);
    let mut actual = fields.keys().map(String::as_str).collect::<Vec<_>>();
    actual.sort_unstable();
    let mut wanted = expected.to_vec();
    wanted.sort_unstable();
    assert_eq!(actual, wanted, "{context} top-level field set");
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

/// Walk one projected resource node (and its children) asserting every
/// `mode` leaf renders as four octal digits.
fn assert_node_modes(node: &Value, context: &str) {
    assert_mode(&node["mode"], context);
    let children = node["children"]
        .as_array()
        .unwrap_or_else(|| panic!("{context} children is not an array"));
    for (index, child) in children.iter().enumerate() {
        assert_node_modes(child, &format!("{context} child {index}"));
    }
}

/// Point the shell's profile at the bootstrap admin over TLS-TCP.
fn use_admin_profile(fixture: &ShellFixture) {
    let text = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string());
    overwrite_profile(fixture, &text);
}

// ── Tests ────────────────────────────────────────────────────────────

/// Golden structure of the three rights views for a principal that holds
/// the admin rights the sibling suites rely on: `user list` carries the
/// seeded shell user, `group tree` exposes the resource ROOT with the two
/// principal lists, and `group members` exposes only the group list.
#[test]
fn user_list_and_group_tree_golden_for_manage_root() {
    let secure = boot_secure("user_list_and_group_tree_golden_for_manage_root");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);

    fixture.guard.mark("rights: user list");
    let users = data(&run_shell(fixture, &["user", "list"]), "user");
    keys_exact(&users, &["users"], "user list");
    let entries = users["users"].as_array().expect("users array");
    for entry in entries {
        keys_exact(
            entry,
            &["name", "principal64", "superuser", "database"],
            "user list entry",
        );
        assert!(
            entry["superuser"].is_boolean(),
            "user list entry `superuser` is not a boolean"
        );
        assert!(
            leaf_u64(&entry["principal64"]).is_some(),
            "user list entry `principal64` is not a decimal string"
        );
        assert!(
            entry["database"].is_null() || entry["database"].is_string(),
            "user list entry `database` is neither null nor a string"
        );
    }
    assert!(
        entries.iter().any(|entry| entry["name"] == SHELL_USER),
        "user list must contain the seeded shell user"
    );

    fixture.guard.mark("rights: group tree");
    let tree = data(&run_shell(fixture, &["group", "tree"]), "group");
    keys_exact(
        &tree,
        &["database_filter", "resources", "functions", "principals"],
        "group tree",
    );
    assert!(
        tree["database_filter"].is_null(),
        "an unfiltered tree must report a null database_filter"
    );
    let resources = &tree["resources"];
    keys_exact(
        resources,
        &[
            "name",
            "kind",
            "owner",
            "owner_name",
            "group",
            "group_name",
            "mode",
            "setuid",
            "children",
        ],
        "group tree resources",
    );
    assert_eq!(
        resources["kind"], "root",
        "the resource root must render kind `root`"
    );
    assert!(
        resources["setuid"].is_boolean(),
        "the resource root `setuid` is not a boolean"
    );
    assert_node_modes(resources, "group tree resources");
    let databases = resources["children"]
        .as_array()
        .expect("resource children array");
    assert!(
        databases
            .iter()
            .any(|child| child["kind"] == "database" && child["name"] == DB_NAME),
        "the seeded database must appear under the resource root"
    );

    let functions = tree["functions"]
        .as_array()
        .expect("group tree functions array");
    for node in functions {
        assert_mode(&node["mode"], "group tree function");
    }
    let principals = object(&tree["principals"], "group tree principals");
    let mut principal_keys = principals.keys().map(String::as_str).collect::<Vec<_>>();
    principal_keys.sort_unstable();
    assert_eq!(
        principal_keys,
        vec!["groups", "users"],
        "group tree principals field set"
    );
    let principal_users = principals["users"]
        .as_array()
        .expect("principals.users array");
    assert!(
        principal_users
            .iter()
            .all(|entry| entry["id"].is_string() && entry["name"].is_string()),
        "principals.users entries must carry decimal-string ids and names"
    );
    assert!(
        principals["groups"].is_array(),
        "principals.groups must be an array"
    );

    fixture.guard.mark("rights: group members");
    let members = data(&run_shell(fixture, &["group", "members"]), "group");
    keys_exact(&members, &["groups"], "group members");
    let groups = members["groups"].as_array().expect("groups array");
    for group in groups {
        keys_exact(group, &["id", "name", "members"], "group members entry");
        assert!(
            leaf_u64(&group["id"]).is_some(),
            "group members entry `id` is not a decimal string"
        );
        let roster = group["members"]
            .as_array()
            .expect("group members entry `members` array");
        for member in roster {
            keys_exact(member, &["id", "name"], "group members entry member");
        }
    }
}

/// The `--db` filter narrows the VIEW only: the restricted principal is
/// refused with or without it, while the admin principal succeeds both ways
/// and reports `database_filter: null` vs the database name.
#[test]
fn group_tree_db_filter_does_not_narrow_permission() {
    let secure = boot_secure("group_tree_db_filter_does_not_narrow_permission");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    let user_password = seed_shell_user(fixture);

    // The restricted profile replaces admin credentials; both probes run on
    // the SAME server as the admin controls below.
    let restricted = scram_profile(fixture, "tls-tcp", &fixture.tls_addr.to_string())
        .replace("username: admin\n", &format!("username: {SHELL_USER}\n"));
    overwrite_profile(fixture, &restricted);
    for args in [
        &["group", "tree"][..],
        &["group", "tree", "--db", DB_NAME][..],
    ] {
        fixture
            .guard
            .mark(format!("restricted: {}", args.join(" ")));
        let run = run_with_secret(fixture, user_password.clone(), false, args);
        assert!(
            !run.contains_canary(user_password.as_str()),
            "the restricted user's secret leaked"
        );
        refusal(&run, "group", "permission_denied", 4);
    }

    use_admin_profile(fixture);
    let unfiltered = data(&run_shell(fixture, &["group", "tree"]), "group");
    assert!(
        unfiltered["database_filter"].is_null(),
        "the unfiltered tree must report a null database_filter"
    );
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
        filtered["resources"]["kind"], "root",
        "the filtered tree must still select the resource root"
    );
}

/// `rights show` is a purely LOCAL selector over the already-fetched tree:
/// `root` renders `/`, a database renders itself, and an over-long path is
/// the typed `usage` refusal with no result record.
#[test]
fn rights_show_root_is_a_local_selector() {
    let secure = boot_secure("rights_show_root_is_a_local_selector");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    use_admin_profile(fixture);

    let root = data(&run_shell(fixture, &["rights", "show", "root"]), "rights");
    keys_exact(
        &root,
        &[
            "path",
            "name",
            "kind",
            "owner",
            "owner_name",
            "group",
            "group_name",
            "mode",
            "setuid",
        ],
        "rights show root",
    );
    assert_eq!(root["path"], "/", "`root` must normalize to `/`");
    assert_eq!(
        root["kind"], "root",
        "`rights show root` must select the root"
    );
    assert!(
        leaf_u64(&root["owner"]).is_some(),
        "rights show root `owner` is not a decimal string"
    );
    assert_mode(&root["mode"], "rights show root");

    let database = data(&run_shell(fixture, &["rights", "show", DB_NAME]), "rights");
    assert_eq!(
        database["kind"], "database",
        "`rights show <database>` must select the database node"
    );
    assert_eq!(
        database["path"], DB_NAME,
        "the database path must echo the selector"
    );
    assert_eq!(database["name"], DB_NAME, "the selected node name");

    // Four slash segments is an over-long selector: refused locally with
    // ONE error envelope, never a partial result record.
    fixture.guard.mark("rights: over-long selector");
    let run = run_shell(fixture, &["rights", "show", "aa/bb/cc/dd"]);
    let envelope = refusal(&run, "rights", "usage", 1);
    assert!(
        envelope["data"].is_null(),
        "a usage refusal must carry no data leaf"
    );
}

/// §3.3: the rights commands are SCRAM only — every one of them is refused
/// in a token session with the fixed subset message and exit 4, and the
/// counting relay in front of the HTTPS listener observes ZERO accepted
/// connections for the whole set.
///
/// Mutants caught: a rights command that re-entered the token allowlist (its
/// dial is counted), and a refusal that dials first and discards the result
/// (the same counter). The relay's positive control — it reports `1` for a
/// real token dial — lives in
/// `state_data::token_subset::token_subset_zero_dial_proven_by_relay_counter`.
#[test]
fn token_session_refuses_rights_commands() {
    let secure = boot_secure("token_session_refuses_rights_commands");
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
    let relay = Relay::start(fixture, secure.http_addr);
    write_secure_profile(fixture, "https", relay.addr, DB_NAME, Some(&secure.ca_path));
    for args in [
        &["user", "list"][..],
        &["group", "tree"][..],
        &["group", "members"][..],
        &["rights", "show", DB_NAME][..],
    ] {
        fixture.guard.mark(format!("token: {}", args.join(" ")));
        let run = run_token_shell(fixture, &token, args);
        let command_part = match args[0] {
            "user" => "user",
            "rights" => "rights",
            _ => "group",
        };
        let value = refusal(&run, command_part, "token_request_denied", 4);
        assert!(
            value["message"].as_str() == Some(TOKEN_SUBSET),
            "wrong fixed token-subset message for `{}`",
            args.join(" ")
        );
    }
    assert_eq!(
        relay.finish(fixture),
        0,
        "a refused rights command must not open a downstream connection"
    );
}
