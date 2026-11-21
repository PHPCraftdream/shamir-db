//! R5 PR2 acceptance against the real listener and production shell binary.

use std::time::Duration;

use serde_json::Value;
use shamir_client::{
    ApiToken, Client, ClientError, TokenEndpoint, TokenHttpClient, TokenHttpOptions, TokenTrust,
    TokenWsOptions,
};
use shamir_query_builder::token_admin::{create_api_token, CreateApiToken};
use shamir_query_builder::{batch::Batch, ddl};
use shamir_query_types::admin::ResourceRef;
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use zeroize::Zeroizing;

use crate::shell_tests::harness::{
    overwrite_profile, run_shell, seed_catalogue, seed_shell_user, ShellFixture, ShellRun, DB_NAME,
    FN_NAME, INDEXED_TABLE, INDEX_NAME, INDEX_ODD_NAME, PLAIN_TABLE, REPO_NAME, SCHEMA_TABLE,
    SHELL_USER,
};
use crate::shell_tests::secure_fixture::{
    boot_secure, boot_secure_root_list, provision_token, revoke_fixture_token, run_token_shell,
    run_with_secret, set_acl_mode, write_secure_profile, SecureFixture,
};

use super::capture::{
    assert_clean, assert_member, error, result, scripted_child, CONNECT_FAILURE, DENIED,
    SERVER_FAILURE, TOKEN_SUBSET,
};

fn ping_and_connect(run: &impl Fn(&[&str]) -> ShellRun) {
    let connected = result(&run(&["connect"]), "connect", None);
    assert!(connected["data"]["connected"].as_bool() == Some(true));
    result(&run(&["ping"]), "ping", Some("Ping"));
}

fn db_list(run: &impl Fn(&[&str]) -> ShellRun) {
    let value = result(&run(&["db", "list"]), "db list", Some("List"));
    assert_member(&value["data"]["databases"], DB_NAME);
}

fn shared_catalogue(run: &impl Fn(&[&str]) -> ShellRun) {
    let repos = result(&run(&["repo", "list"]), "repo list", Some("List"));
    assert_member(&repos["data"]["repos"], REPO_NAME);
    let tables = result(
        &run(&["table", "list", REPO_NAME]),
        "table list",
        Some("List"),
    );
    for table in [PLAIN_TABLE, INDEXED_TABLE, SCHEMA_TABLE] {
        assert_member(&tables["data"]["tables"], table);
    }
    let indexes = result(
        &run(&["index", "list", REPO_NAME, INDEXED_TABLE]),
        "index list",
        Some("List"),
    );
    let indexes = indexes["data"]["indexes"]
        .as_array()
        .expect("indexes array");
    let escaped_odd = INDEX_ODD_NAME
        .replace('\t', "\\u0009")
        .replace('\x1b', "\\u001B")
        .replace('\x07', "\\u0007");
    for name in [INDEX_NAME, escaped_odd.as_str()] {
        assert!(
            indexes.iter().any(|row| row["name"].as_str() == Some(name)),
            "missing projected index name"
        );
    }
    let functions = result(&run(&["function", "list"]), "function list", Some("List"));
    let functions = functions["data"]["functions"]
        .as_array()
        .expect("functions array");
    assert!(
        functions.iter().any(
            |row| row["name"].as_str() == Some(FN_NAME) && row["kind"].as_str() == Some("wasm")
        ),
        "missing seeded function"
    );
    let described = result(
        &run(&["table", "describe", REPO_NAME, SCHEMA_TABLE]),
        "table describe",
        Some("DescribeTable"),
    );
    let schema = result(
        &run(&["table", "schema", REPO_NAME, SCHEMA_TABLE]),
        "table schema",
        Some("GetTableSchema"),
    );
    assert!(schema["data"]["get_table_schema"].as_str() == Some(SCHEMA_TABLE));
    for value in [&described, &schema] {
        let rules = value["data"]["schema"].as_array().expect("schema rules");
        assert!(
            rules.iter().any(|rule| {
                rule["path"]
                    .as_array()
                    .is_some_and(|path| path.len() == 1 && path[0].as_str() == Some("score"))
                    && rule["type"].as_str() == Some("int")
                    && rule["required"].as_bool() == Some(true)
            }),
            "missing seeded score rule"
        );
    }
}

fn repo_description(run: &impl Fn(&[&str]) -> ShellRun) {
    let value = result(
        &run(&["repo", "describe", REPO_NAME]),
        "repo describe",
        Some("DescribeRepo"),
    );
    let data = value["data"].as_object().expect("repo description object");
    assert!(data["describe_repo"].as_str() == Some(REPO_NAME));
    assert!(data["db"].as_str() == Some(DB_NAME));
    assert!(
        !data.contains_key("repo"),
        "the wire repo field denotes the database"
    );
    // An unbuffered Fjall repository is recorded in the catalogue as `fjall_raw`.
    assert!(
        matches!(data["engine"].as_str(), Some("fjall" | "fjall_raw")),
        "repo describe must report the persistent engine"
    );
    assert!(
        data["durability"].is_object(),
        "missing effective durability"
    );
    assert!(data.get("durability_override").is_some_and(Value::is_null));
    for field in ["path", "lineage_id", "history_epoch"] {
        assert!(!data.contains_key(field), "unexpected peer field: {field}");
    }
}

const MISSING_REPO: &str = "missing_repo_reflection_marker";

#[test]
fn shell_secure_native_wss_scram_catalogue() {
    let fixture = boot_secure("shell_secure_native_wss_scram_catalogue");
    let shell = &fixture.shell;
    seed_catalogue(shell);
    let password = seed_shell_user(shell);
    let profile = write_secure_profile(shell, "wss", fixture.wss_addr, DB_NAME, None);
    let admin_run = |args: &[&str]| {
        let run = run_shell(shell, args);
        assert_clean(&run, shell.canary.as_str());
        run
    };
    ping_and_connect(&admin_run);
    db_list(&admin_run);
    shared_catalogue(&admin_run);
    repo_description(&admin_run);

    let profile_text = std::fs::read_to_string(&profile).expect("read pinned profile");
    let limited = profile_text.replace("username: admin", &format!("username: {SHELL_USER}"));
    assert!(limited != profile_text, "change the profile principal");
    overwrite_profile(shell, &limited);
    let user_run = |args: &[&str]| {
        let run = run_with_secret(shell, password.clone(), false, args);
        assert_clean(&run, password.as_str());
        assert!(!run.contains_canary(shell.canary.as_str()));
        run
    };
    ping_and_connect(&user_run);
    // Give the user the routing-database floor. DescribeRepo must still
    // reject the same command that succeeded under admin SCRAM above.
    grant_database_access(&fixture);
    let repos = result(&user_run(&["repo", "list"]), "repo list", Some("List"));
    assert_member(&repos["data"]["repos"], REPO_NAME);
    error(
        &user_run(&["repo", "describe", REPO_NAME]),
        "repo describe",
        4,
        "permission_denied",
        DENIED,
    );
}

#[test]
fn shell_secure_native_wss_scram_reflection_and_pin_refusal() {
    let fixture = boot_secure("shell_secure_native_wss_scram_reflection_and_pin_refusal");
    let shell = &fixture.shell;
    seed_catalogue(shell);
    let profile = write_secure_profile(shell, "wss", fixture.wss_addr, DB_NAME, None);
    let admin_run = |args: &[&str]| {
        let run = run_shell(shell, args);
        assert_clean(&run, shell.canary.as_str());
        run
    };

    // The real DescribeRepo handler includes the missing name in its error.
    let missing = admin_run(&["repo", "describe", MISSING_REPO]);
    error(&missing, "repo describe", 5, "server_error", SERVER_FAILURE);
    assert!(
        !missing.contains_canary(MISSING_REPO),
        "opaque peer error reflected"
    );
    assert_human_reflection(&fixture.shell, &fixture.shell.canary);

    let profile_text = std::fs::read_to_string(&profile).expect("read pinned profile");
    let bad_pin = profile_text.replacen(&shell.pin_hex(), &"00".repeat(32), 1);
    assert!(bad_pin != profile_text, "change the enrolled pin");
    overwrite_profile(shell, &bad_pin);
    error(
        &admin_run(&["connect"]),
        "connect",
        6,
        "pin_mismatch",
        "server identity pin does not match the verified pin in the profile",
    );
}

fn grant_database_access(fixture: &SecureFixture) {
    set_acl_mode(
        &fixture.shell,
        ResourceRef::Database {
            database: DB_NAME.into(),
        },
        0o755,
    );
}

fn token_request(label: &str) -> CreateApiToken {
    create_api_token(label)
        .for_user(SHELL_USER)
        .mode(ApiTokenMode::ReadOnly)
        .channels(ApiTokenChannels::Both)
}

#[test]
fn shell_secure_https_and_token_wss_catalogue() {
    let fixture = boot_secure("shell_secure_https_and_token_wss_catalogue");
    let shell = &fixture.shell;
    seed_catalogue(shell);
    drop(seed_shell_user(shell));
    grant_database_access(&fixture);
    for resource in [
        ResourceRef::Store {
            store: [DB_NAME.into(), REPO_NAME.into()],
        },
        ResourceRef::Table {
            table: [DB_NAME.into(), REPO_NAME.into(), INDEXED_TABLE.into()],
        },
        ResourceRef::Table {
            table: [DB_NAME.into(), REPO_NAME.into(), SCHEMA_TABLE.into()],
        },
        ResourceRef::FunctionNamespace {
            function_namespace: true,
        },
    ] {
        set_acl_mode(shell, resource, 0o755);
    }
    let scoped = provision_token(
        shell,
        token_request("scoped-catalogue").databases([DB_NAME]),
    );
    let unscoped = provision_token(shell, token_request("unscoped-catalogue"));
    let wrong_ca = tempfile::TempDir::new().expect("unrelated CA directory");
    let wrong_ca_path = wrong_ca.path().join("unrelated.pem");
    let (pem, _key) =
        shamir_transport_tcp::tls::generate_self_signed_server_cert(vec!["localhost".into()])
            .expect("generate unrelated valid PEM");
    std::fs::write(&wrong_ca_path, &pem).expect("write unrelated public PEM");

    for transport in ["https", "token-wss"] {
        shell.guard.mark(transport);
        let profile = write_secure_profile(
            shell,
            transport,
            fixture.http_addr,
            DB_NAME,
            Some(&fixture.ca_path),
        );
        let scoped_run = |args: &[&str]| {
            let run = run_token_shell(shell, &scoped, args);
            assert_clean(&run, scoped.as_str());
            run
        };
        ping_and_connect(&scoped_run);
        shared_catalogue(&scoped_run);
        assert_token_repl(&fixture.shell, scoped.clone());
        // Same command, same principal, same routing DB. The scoped token
        // hits the perimeter denial; the unscoped token reaches Root ACL.
        error(
            &scoped_run(&["db", "list"]),
            "db list",
            4,
            "permission_denied",
            DENIED,
        );
        let unscoped_run = |args: &[&str]| {
            let run = run_token_shell(shell, &unscoped, args);
            assert_clean(&run, unscoped.as_str());
            run
        };
        result(&unscoped_run(&["repo", "list"]), "repo list", Some("List"));
        error(
            &unscoped_run(&["db", "list"]),
            "db list",
            4,
            "permission_denied",
            DENIED,
        );
        for (token, server_code) in [
            (&scoped, "token_database_denied"),
            (&unscoped, "access_denied"),
        ] {
            assert_server_db_denial(&fixture, token, transport, server_code);
        }

        let trusted_text = std::fs::read_to_string(profile).expect("read trusted token profile");
        let wrong_host = trusted_text.replace("sni: localhost", "sni: wrong-name.invalid");
        assert!(
            wrong_host != trusted_text,
            "change the certificate name oracle"
        );
        overwrite_profile(shell, &wrong_host);
        error(
            &scoped_run(&["connect"]),
            "connect",
            2,
            "connection_failed",
            CONNECT_FAILURE,
        );
        write_secure_profile(
            shell,
            transport,
            fixture.http_addr,
            DB_NAME,
            Some(&wrong_ca_path),
        );
        error(
            &scoped_run(&["connect"]),
            "connect",
            2,
            "connection_failed",
            CONNECT_FAILURE,
        );
        overwrite_profile(shell, &trusted_text);
        ping_and_connect(&scoped_run);
    }

    // A real, correctly formatted token is revoked through the typed admin
    // operation after the positive controls, then rejected on both listeners.
    let id = ApiToken::new(scoped.as_str())
        .expect("fixture token format")
        .id();
    revoke_fixture_token(shell, &id);
    for transport in ["https", "token-wss"] {
        write_secure_profile(
            shell,
            transport,
            fixture.http_addr,
            DB_NAME,
            Some(&fixture.ca_path),
        );
        let run = run_token_shell(shell, &scoped, &["connect"]);
        assert_clean(&run, scoped.as_str());
        error(
            &run,
            "connect",
            3,
            "authentication_failed",
            "authentication failed",
        );
    }
}

#[test]
fn shell_token_db_list_requires_unscoped_token_with_root_list() {
    let fixture = boot_secure_root_list("shell_token_db_list_with_root_list");
    let shell = &fixture.shell;
    seed_catalogue(shell);
    drop(seed_shell_user(shell));
    grant_database_access(&fixture);
    let unscoped = provision_token(shell, token_request("root-list-unscoped"));
    let scoped = provision_token(
        shell,
        token_request("root-list-scoped").databases([DB_NAME]),
    );

    for transport in ["https", "token-wss"] {
        shell.guard.mark(transport);
        write_secure_profile(
            shell,
            transport,
            fixture.http_addr,
            DB_NAME,
            Some(&fixture.ca_path),
        );
        let run = |token: &Zeroizing<String>, args: &[&str]| {
            let run = run_token_shell(shell, token, args);
            assert_clean(&run, token.as_str());
            run
        };
        result(&run(&unscoped, &["ping"]), "ping", Some("Ping"));
        db_list(&|args| run(&unscoped, args));

        // Same Root ACL, owner and route: only the token scope changes.
        result(&run(&scoped, &["ping"]), "ping", Some("Ping"));
        error(
            &run(&scoped, &["db", "list"]),
            "db list",
            4,
            "permission_denied",
            DENIED,
        );
        assert_server_db_denial(&fixture, &scoped, transport, "token_database_denied");
    }
}

// Both server failures intentionally collapse to the same shell code. Use
// the typed SDK with the same token/route to pin the distinct server reasons.
fn assert_server_db_denial(
    fixture: &SecureFixture,
    token: &str,
    transport: &str,
    server_code: &'static str,
) {
    const BOUND: Duration = Duration::from_secs(120);
    let pem = std::fs::read(&fixture.ca_path).expect("fixture trust anchor");
    let trust = TokenTrust::pem(&pem).expect("fixture CA parses");
    let endpoint = TokenEndpoint::https(fixture.http_addr, "localhost", trust);
    let token = ApiToken::new(token).expect("fixture token parses");
    let mut batch = Batch::new();
    batch.list_databases("databases", ddl::list_databases());
    let batch = batch.build();
    let rest = transport == "https";
    fixture.shell.rt.run(async move {
        tokio::time::timeout(BOUND, async move {
            let reply = if rest {
                let client = TokenHttpClient::connect_once(TokenHttpOptions::new(endpoint, token))
                    .await
                    .expect("typed HTTPS control connects");
                client.execute(DB_NAME, batch).await
            } else {
                let client = Client::connect_token_ws(TokenWsOptions::new(endpoint, token))
                    .await
                    .expect("typed token WSS control connects");
                let reply = client.execute(DB_NAME, batch).await;
                client.close().await;
                reply
            };
            assert!(
                matches!(reply, Err(ClientError::Db { code, .. }) if code == server_code),
                "server did not return the expected denial: {server_code}"
            );
        })
        .await
        .expect("typed server-denial control finishes within fixture bound");
    });
}

fn assert_human_reflection(shell: &ShellFixture, password: &Zeroizing<String>) {
    let script =
        format!("index list {REPO_NAME} {INDEXED_TABLE}\nrepo describe {MISSING_REPO}\nquit\n");
    let run = scripted_child(shell, password, &script);
    assert_eq!(run.exit_code, 0, "scripted REPL reaches quit");
    assert!(
        !run.contains_canary(MISSING_REPO),
        "opaque peer error in human output"
    );
    let human = String::from_utf8_lossy(&run.stderr);
    assert!(
        human.contains("odd\\u005C\\u005Cu0009"),
        "escaped index name must reach human output"
    );
    assert!(
        human.contains("server_error") && human.contains(SERVER_FAILURE),
        "expected the local repo failure"
    );
}

fn assert_token_repl(shell: &ShellFixture, token: Zeroizing<String>) {
    let script = format!("connect --auth token\nping\nrepo describe {REPO_NAME}\nping\nquit\n");
    let run = scripted_child(shell, &token, &script);
    assert_eq!(run.exit_code, 0, "scripted REPL reaches quit");
    let (_, suffix) = token.rsplit_once('.').expect("issued bearer format");
    assert!(!run.contains_canary(suffix), "token suffix in human output");
    let human = String::from_utf8_lossy(&run.stderr);
    let outcomes: Vec<_> = human
        .lines()
        .filter(|line| matches!(*line, "connect" | "ping" | "error"))
        .collect();
    assert!(
        outcomes == ["connect", "ping", "error", "ping"],
        "expected token connect, ping, refusal, and another ping in that order"
    );
    assert!(human.contains("token_request_denied") && human.contains(TOKEN_SUBSET));
}
