use crate::audit_appender::FjallAuditAppender;
use crate::config::Config;
use crate::engine::server_meta::ServerMetaStore;
use crate::security::tokens::cli::{
    args::{CreateArgs, ListArgs},
    run_with_io, CliIo, TokenAction,
};
use crate::security::tokens::cli::{
    create::{escape, id_text, parse_ttl},
    list::render_list,
    revoke::parse_id,
    secret_output,
};
use crate::security::tokens::{TokenAuditContext, TokenView};
use crate::security::user_directory::FjallUserDirectory;
use clap::Parser;
use shamir_connect::common::{crypto::StoredKey, kdf_params::KdfParams};
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::token::parse_token;
use shamir_connect::server::{audit_chain::AuditChain, user_record::UserRecord};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;
use zeroize::Zeroizing;

#[derive(Parser)]
struct TestCli {
    #[command(subcommand)]
    action: TokenAction,
}
fn config(root: &TempDir) -> Config {
    let src="data_dir: data\nkdf_defaults: { memory_kb: 19456, time: 2, parallelism: 1, argon2_version: 19 }\nlisteners: [{ kind: tcp, addr: 127.0.0.1:7331, profile: tls_exporter }]\ntls: { cert_path: cert.pem, key_path: key.pem }\naudit: { max_file_size_mb: 0, retention_days: 0 }\n";
    let mut c: Config = ktav::from_str(src).expect("test config");
    c.data_dir = root.path().join("data");
    c
}
fn streams<'a>(o: &'a mut Vec<u8>, e: &'a mut Vec<u8>, terminal: bool) -> CliIo<'a> {
    CliIo {
        stdout: o,
        stderr: e,
        is_terminal: terminal,
    }
}
fn user_record() -> UserRecord {
    UserRecord {
        salt: [7; 16],
        stored_key: StoredKey([8; 32]),
        server_key: Zeroizing::new([9; 32]),
        kdf_params: KdfParams::DEFAULT,
        tickets_invalid_before_ns: 0,
    }
}
#[test]
fn parser_rejects_missing_label_bad_ttl_bad_channel_and_revoke_without_yes() {
    assert!(TestCli::try_parse_from(["s", "create", "--user", "u"]).is_err());
    assert!(
        TestCli::try_parse_from(["s", "create", "--user", "u", "--label", "l", "--ttl", "3x"])
            .is_err()
    );
    assert!(TestCli::try_parse_from([
        "s",
        "create",
        "--user",
        "u",
        "--label",
        "l",
        "--channels",
        "bad"
    ])
    .is_err());
    assert!(TestCli::try_parse_from(["s", "revoke-all"]).is_err());
    assert!(TestCli::try_parse_from(["s", "revoke-all", "--yes"]).is_ok());
    assert_eq!(parse_ttl("30d").expect("ttl"), 2592000);
    assert!(parse_ttl("0s").is_err());
    assert!(parse_id("AAAAAAAAAAAAAAAAAAAAA!").is_err());
    assert!(parse_id("AAAAAAAAAAAAAAAAAAAAAA").is_ok());
}
#[test]
fn stable_list_render_escapes_label_owner_and_database_controls() {
    let tmp = tempfile::tempdir().expect("tmp");
    let users = FjallUserDirectory::open(tmp.path().join("users")).expect("dir");
    let row = TokenView {
        id: [1; 16],
        owner_user_id: [9; 16],
        label: "a\tb".into(),
        created_at_ns: 1,
        expires_at_ns: 2,
        mode: shamir_connect::server::token::TokenMode::ReadOnly,
        channels: shamir_connect::server::token::TokenChannels::Both,
        databases: Some(vec!["db\nx".into()]),
        revoked_at_ns: None,
    };
    let s = render_list(&[row], &users, None);
    assert!(s.starts_with(&format!("{}\ta\\tb\t<unknown>", id_text([1; 16]))));
    assert!(s.contains("db\\nx\t1\t2\t\n"));
    assert!(!s.contains("a\tb"));
    assert_eq!(escape("x\ny"), "x\\ny");
}
#[tokio::test]
async fn no_tty_refusal_precedes_data_directory_open() {
    let tmp = TempDir::new().expect("tmp");
    let c = config(&tmp);
    let mut o = Vec::new();
    let mut e = Vec::new();
    let mut io = streams(&mut o, &mut e, false);
    let a = TokenAction::Create(CreateArgs {
        user: "nope".into(),
        label: "l".into(),
        ttl: None,
        read_write: false,
        channels: "both".into(),
        databases: vec![],
        out_file: None,
    });
    assert!(run_with_io(&c, a, &mut io).await.is_err());
    assert!(!c.data_dir.exists());
    assert!(o.is_empty());
}
#[tokio::test]
async fn existing_secret_file_is_refused_before_data_open() {
    let tmp = TempDir::new().expect("tmp");
    let c = config(&tmp);
    let p = tmp.path().join("keep");
    std::fs::write(&p, b"untouched").expect("seed");
    let mut o = Vec::new();
    let mut e = Vec::new();
    let mut io = streams(&mut o, &mut e, false);
    let a = TokenAction::Create(CreateArgs {
        user: "nope".into(),
        label: "l".into(),
        ttl: None,
        read_write: false,
        channels: "both".into(),
        databases: vec![],
        out_file: Some(p.clone()),
    });
    assert!(run_with_io(&c, a, &mut io).await.is_err());
    assert_eq!(std::fs::read(p).expect("file"), b"untouched");
    assert!(!c.data_dir.exists());
}
#[test]
fn secret_file_is_exclusive_and_unix_mode_0600() {
    let tmp = TempDir::new().expect("tmp");
    let p = tmp.path().join("secret");
    let output = secret_output::prepare(Some(&p), false).expect("new exclusive file");
    let mut o = Vec::new();
    let mut e = Vec::new();
    secret_output::deliver(output, "secret", &mut o, &mut e).expect("deliver");
    assert_eq!(std::fs::read(&p).expect("contents"), b"secret\n");
    #[cfg(unix)]
    {
        let mode = std::fs::metadata(&p)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert!(e.is_empty());
    }
    #[cfg(windows)]
    assert!(String::from_utf8(e)
        .expect("warning")
        .contains("ACLs were not tightened"));
}
#[test]
fn service_audit_context_defaults_to_token_transport() {
    let context = TokenAuditContext::default();
    assert_eq!(context.transport, "token");
    assert!(context.actor_username.is_empty());
    assert_eq!(context.session_id_prefix, [0; 8]);
}
#[tokio::test]
async fn create_list_revoke_reopen_and_verify_audit_continuity() {
    let tmp = TempDir::new().expect("tmp");
    let c = config(&tmp);
    std::fs::create_dir_all(&c.data_dir).expect("data");
    let dir = FjallUserDirectory::open(c.data_dir.join("users")).expect("directory");
    dir.insert("alice".into(), user_record()).expect("user");
    drop(dir);
    let mut o = Vec::new();
    let mut e = Vec::new();
    let a = TokenAction::Create(CreateArgs {
        user: "alice".into(),
        label: "runner".into(),
        ttl: Some(7200),
        read_write: false,
        channels: "both".into(),
        databases: vec!["main".into()],
        out_file: None,
    });
    {
        let mut io = streams(&mut o, &mut e, true);
        run_with_io(&c, a, &mut io).await.expect("create");
    }
    let text = String::from_utf8(o).expect("stdout");
    let secret = text.lines().next().expect("one-time secret");
    assert!(secret.starts_with("shk1."));
    let parsed = parse_token(secret.as_bytes()).expect("canonical token format");
    drop(parsed);
    let token_id = secret.split('.').nth(1).expect("id").to_owned();
    assert!(!e.windows(secret.len()).any(|w| w == secret.as_bytes()));
    assert!(text.contains("label=runner"));
    let mut listed = Vec::new();
    let mut le = Vec::new();
    {
        let mut io = streams(&mut listed, &mut le, false);
        run_with_io(
            &c,
            TokenAction::List(ListArgs {
                user: Some("alice".into()),
            }),
            &mut io,
        )
        .await
        .expect("list");
    }
    assert!(String::from_utf8(listed)
        .expect("listing")
        .contains("runner"));
    let mut revoked = Vec::new();
    let mut re = Vec::new();
    {
        let mut io = streams(&mut revoked, &mut re, false);
        run_with_io(&c, TokenAction::Revoke { id: token_id }, &mut io)
            .await
            .expect("revoke");
    }
    assert_eq!(revoked, b"revoked=1\n");
    let mut user_revoked = Vec::new();
    let mut user_re = Vec::new();
    {
        let mut io = streams(&mut user_revoked, &mut user_re, false);
        run_with_io(
            &c,
            TokenAction::RevokeUser {
                user: "alice".into(),
            },
            &mut io,
        )
        .await
        .expect("revoke user");
    }
    let mut all_revoked = Vec::new();
    let mut all_re = Vec::new();
    {
        let mut io = streams(&mut all_revoked, &mut all_re, false);
        run_with_io(&c, TokenAction::RevokeAll { yes: true }, &mut io)
            .await
            .expect("revoke all");
    }
    let mut again = Vec::new();
    let mut ae = Vec::new();
    {
        let mut io = streams(&mut again, &mut ae, false);
        run_with_io(&c, TokenAction::List(ListArgs { user: None }), &mut io)
            .await
            .expect("reopen list");
    }
    let rows = crate::security::tokens::TokenStore::open(&c.data_dir)
        .await
        .expect("store")
        .scan()
        .await
        .expect("scan");
    let revoked_row = rows
        .iter()
        .find(|r| r.label == "runner")
        .expect("token row");
    assert!(revoked_row.revoked_at_ns.is_some());
    let entries = FjallAuditAppender::read_log_for_verify(&c.data_dir).expect("log");
    let meta = ServerMetaStore::open_or_init(c.data_dir.join("server_meta")).expect("meta reopen");
    let key = meta.audit_chain_key().expect("key");
    AuditChain::verify_chain(&key, &entries).expect("valid chain");
    let cp = meta.audit_checkpoint().expect("checkpoint");
    AuditChain::verify_against_checkpoint(&entries, cp.0, &cp.1).expect("checkpoint verifies");
    assert!(entries.iter().any(|x| x.event == "api_token_created"));
    assert!(entries.iter().any(|x| x.event == "api_token_revoked"));
    for entry in entries.iter().filter(|x| {
        matches!(
            x.event.as_str(),
            "api_token_created" | "api_token_revoked" | "api_token_user_revoked"
        )
    }) {
        assert_eq!(entry.transport, "cli");
        assert!(entry.user.is_empty());
        assert_eq!(entry.session_id_prefix, [0; 8]);
        assert!(entry.ip_subnet.is_empty());
    }
    assert_eq!(
        entries
            .iter()
            .filter(|x| x.event == "api_token_user_revoked")
            .count(),
        2
    );
    drop(meta);
    let reopened = FjallUserDirectory::open(c.data_dir.join("users")).expect("users second open");
    assert!(reopened
        .list_all()
        .expect("list users")
        .iter()
        .any(|(_, u)| u.username == "alice"));
}
