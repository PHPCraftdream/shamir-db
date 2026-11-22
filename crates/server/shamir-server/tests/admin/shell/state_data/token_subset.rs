//! R5 PR3 shell state/data tests: the §3.3 token read-only subset, proven by a counting relay. Drives the REAL production
//! shell through the shared helpers of [`super::state_data_common`].

use serde_json::Value;
use shamir_query_builder::token_admin;
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use zeroize::Zeroizing;

use super::harness::{
    seed_catalogue, seed_shell_user, ShellRun, ADMIN, DB_NAME, PLAIN_TABLE, REPO_NAME,
    SCHEMA_TABLE, SHELL_USER,
};
use super::secure_fixture::{
    boot_secure, provision_token, run_token_shell, set_acl_mode, write_secure_profile,
};
use super::state_data_common::{data, insert_doc, insert_rows, refusal, Relay, TOKEN_SUBSET};
use shamir_query_types::admin::ResourceRef;

#[test]
fn token_subset_zero_dial_proven_by_relay_counter() {
    let secure = boot_secure("token_subset_zero_dial_proven_by_relay_counter");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);
    // Read-only API tokens do not get the server's superuser bypass.
    // Explicitly grant OTHER-read at every ancestor and the addressed
    // table so the positive count/head probes demonstrate real ACL
    // authorization, not a privileged owner assumption.
    set_acl_mode(
        fixture,
        ResourceRef::Database {
            database: DB_NAME.into(),
        },
        0o755,
    );
    set_acl_mode(
        fixture,
        ResourceRef::Store {
            store: [DB_NAME.into(), REPO_NAME.into()],
        },
        0o755,
    );
    set_acl_mode(
        fixture,
        ResourceRef::Table {
            table: [DB_NAME.into(), REPO_NAME.into(), PLAIN_TABLE.into()],
        },
        0o755,
    );
    // A REAL, well-formed key: the outside-subset `data get` probe must
    // pass the shell's own Base58 validation and be refused by the token
    // gate, not by the local `invalid_record_id` check.
    let key_id = insert_rows(fixture, 1).pop().expect("seeded probe key");
    for transport in ["https", "token-wss"] {
        fixture.guard.mark(transport);
        // Two scoped tokens: the no-role limited user (ACL denials) and
        // the bootstrap admin (the positive in-subset success path).
        let provision = |user: &str| {
            provision_token(
                fixture,
                token_admin::create_api_token(transport)
                    .for_user(user)
                    .databases([DB_NAME])
                    .mode(ApiTokenMode::ReadOnly)
                    .channels(ApiTokenChannels::Both),
            )
        };
        let token = provision(SHELL_USER);
        let admin_token = provision(ADMIN);
        let probe = |token: &Zeroizing<String>, args: &[&str]| {
            let relay = Relay::start(fixture, secure.http_addr);
            write_secure_profile(
                fixture,
                transport,
                relay.addr,
                DB_NAME,
                Some(&secure.ca_path),
            );
            let run = run_token_shell(fixture, token, args);
            let accepted = relay.finish(fixture);
            (run, accepted)
        };

        // Positive control: same TLS target, profile shape and token.
        let (positive, accepted) = probe(&admin_token, &["ping"]);
        assert_eq!(positive.exit_code, 0, "{transport} positive control failed");
        assert_eq!(accepted, 1, "{transport} positive control missed relay");

        // Outside the subset: refused client-side, exit 4, zero dials.
        for (args, command_part) in [
            (&["status"][..], "status"),
            (&["version", "show"][..], "version"),
            (
                &[
                    "data",
                    "get",
                    DB_NAME,
                    REPO_NAME,
                    PLAIN_TABLE,
                    key_id.as_str(),
                ][..],
                "get",
            ),
            (&["replication", "subscriptions"][..], "subscriptions"),
        ] {
            let (run, accepted) = probe(&token, args);
            assert_eq!(
                accepted, 0,
                "{transport} {command_part} dialed before token refusal"
            );
            let value = refusal(&run, command_part, "token_request_denied", 4);
            assert!(
                value["message"].as_str() == Some(TOKEN_SUBSET),
                "wrong fixed token-subset message"
            );
        }

        // In the subset: both scoped tokens reach the wire and read the
        // one seeded row. The limited user's read is authorized by the
        // explicit OTHER-read ACL grants above; token sessions do not
        // inherit any superuser bypass. Exact payload assertions keep
        // this from passing on an empty result or a token-gate denial.
        let envelope = |run: &ShellRun| {
            let line = run
                .stdout
                .strip_suffix(b"\n")
                .expect("LF-terminated output");
            let value: Value =
                serde_json::from_slice(line).unwrap_or_else(|_| panic!("invalid JSON"));
            assert_eq!(value["version"].as_u64(), Some(1), "wrong wire version");
            value
        };
        for args in [
            &["data", "count", DB_NAME, REPO_NAME, PLAIN_TABLE][..],
            &["data", "head", DB_NAME, REPO_NAME, PLAIN_TABLE, "3"][..],
        ] {
            let (admin_run, admin_accepted) = probe(&admin_token, args);
            let (limited_run, limited_accepted) = probe(&token, args);
            assert_eq!(admin_accepted, 1, "{transport} admin token did not dial");
            assert_eq!(
                limited_accepted, 1,
                "{transport} limited token did not dial"
            );
            for (run, label) in [(&admin_run, "admin-owned"), (&limited_run, "limited-user")] {
                assert_eq!(run.exit_code, 0, "{transport} {label} scoped token failed");
                let value = envelope(run);
                assert_eq!(value["kind"].as_str(), Some("result"));
                assert!(value["data"].is_object(), "in-subset read lost its payload");
                match args[1] {
                    "count" => {
                        assert_eq!(value["data"]["count"].as_str(), Some("1"));
                        assert!(value["data"]["count_path"].as_str().is_some());
                    }
                    "head" => {
                        assert_eq!(value["data"]["rows"].as_array().map(Vec::len), Some(1));
                        assert_eq!(value["data"]["rows_returned"].as_str(), Some("1"));
                    }
                    other => panic!("unexpected in-subset command {other}"),
                }
            }
        }
    }
}

#[test]
fn token_database_read_table_read_denied() {
    let secure = boot_secure("token_database_read_table_read_denied");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    seed_shell_user(fixture);

    // The child has OTHER-read on the named database, so Database Read
    // succeeds; the addressed table is owner-only, so the second
    // Table-Read authorization fails. This is distinct from the no-role
    // case (which can fail at the first, Database check).
    set_acl_mode(
        fixture,
        ResourceRef::Database {
            database: DB_NAME.into(),
        },
        0o755,
    );
    set_acl_mode(
        fixture,
        ResourceRef::Table {
            table: [DB_NAME.into(), REPO_NAME.into(), PLAIN_TABLE.into()],
        },
        0o700,
    );
    // Positive ACL control: the same user/token can read a different table
    // in the same database after the database-level grant. This establishes
    // that Database Read succeeds and that only the target table's missing
    // Other-Read bit causes the next refusals.
    set_acl_mode(
        fixture,
        ResourceRef::Store {
            store: [DB_NAME.into(), REPO_NAME.into()],
        },
        0o755,
    );
    set_acl_mode(
        fixture,
        ResourceRef::Table {
            table: [DB_NAME.into(), REPO_NAME.into(), SCHEMA_TABLE.into()],
        },
        0o755,
    );
    let token = provision_token(
        fixture,
        token_admin::create_api_token("https")
            .for_user(SHELL_USER)
            .databases([DB_NAME])
            .mode(ApiTokenMode::ReadOnly)
            .channels(ApiTokenChannels::Both),
    );
    write_secure_profile(
        fixture,
        "https",
        secure.http_addr,
        DB_NAME,
        Some(&secure.ca_path),
    );
    let control = run_token_shell(
        fixture,
        &token,
        &["data", "count", DB_NAME, REPO_NAME, SCHEMA_TABLE],
    );
    assert_eq!(
        data(&control, "count")["count"].as_str(),
        Some("0"),
        "database read + table read positive ACL control must pass"
    );
    for args in [
        &["data", "count", DB_NAME, REPO_NAME, PLAIN_TABLE][..],
        &["data", "head", DB_NAME, REPO_NAME, PLAIN_TABLE, "3"][..],
    ] {
        write_secure_profile(
            fixture,
            "https",
            secure.http_addr,
            DB_NAME,
            Some(&secure.ca_path),
        );
        let run = run_token_shell(fixture, &token, args);
        refusal(&run, "data", "permission_denied", 4);
    }
}

/// A separate fresh fixture makes the credential-reflection probe
/// non-vacuous without changing the exactly-one-row count/head positive
/// controls in `token_subset_zero_dial_proven_by_relay_counter`.
#[test]
fn token_data_head_credential_reflection_fails_closed() {
    let secure = boot_secure("token_data_head_credential_reflection_fails_closed");
    let fixture = &secure.shell;
    seed_catalogue(fixture);
    // Explicit non-superuser read ACLs let the token reach the actual
    // `data head` result; without these, permission_denied would make the
    // credential-reflection assertion vacuous.
    set_acl_mode(
        fixture,
        ResourceRef::Database {
            database: DB_NAME.into(),
        },
        0o755,
    );
    set_acl_mode(
        fixture,
        ResourceRef::Store {
            store: [DB_NAME.into(), REPO_NAME.into()],
        },
        0o755,
    );
    set_acl_mode(
        fixture,
        ResourceRef::Table {
            table: [DB_NAME.into(), REPO_NAME.into(), PLAIN_TABLE.into()],
        },
        0o755,
    );
    let token = provision_token(
        fixture,
        token_admin::create_api_token("https")
            .for_user(ADMIN)
            .databases([DB_NAME])
            .mode(ApiTokenMode::ReadOnly)
            .channels(ApiTokenChannels::Both),
    );
    let secret = Zeroizing::new(token.as_str().to_owned());
    let key = insert_doc(fixture, "note", &secret);
    assert!(!key.is_empty(), "hostile record needs a server-minted key");

    // Positive control proves the scoped token, TLS profile and relay work.
    let relay = Relay::start(fixture, secure.http_addr);
    write_secure_profile(fixture, "https", relay.addr, DB_NAME, Some(&secure.ca_path));
    let positive = run_token_shell(fixture, &token, &["ping"]);
    assert_eq!(positive.exit_code, 0, "token positive control failed");
    assert_eq!(
        relay.finish(fixture),
        1,
        "token positive control did not dial"
    );

    // A real credential string in a real authorized record must be caught
    // by the payload guard before output. The bounded `data head` request
    // returns the row; without the guard the payload contains `secret`.
    let relay = Relay::start(fixture, secure.http_addr);
    write_secure_profile(fixture, "https", relay.addr, DB_NAME, Some(&secure.ca_path));
    let run = run_token_shell(
        fixture,
        &token,
        &["data", "head", DB_NAME, REPO_NAME, PLAIN_TABLE, "10"],
    );
    assert_eq!(relay.finish(fixture), 1, "reflection probe did not dial");
    assert!(
        !run.stdout
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()),
        "the token secret leaked through data head"
    );
    let envelope: Value = serde_json::from_slice(
        run.stdout
            .strip_suffix(b"\n")
            .expect("LF-terminated output"),
    )
    .expect("reflection probe emitted invalid JSON");
    assert_eq!(envelope["kind"].as_str(), Some("error"));
    assert_eq!(envelope["code"].as_str(), Some("projection_limit"));
    assert_eq!(run.exit_code, 5, "reflection refusal is projection exit 5");
}
