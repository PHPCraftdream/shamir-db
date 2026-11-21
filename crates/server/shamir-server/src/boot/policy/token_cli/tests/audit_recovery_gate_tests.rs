//! End-to-end CLI regressions for review findings 2 and 4 of
//! `docs/dev-artifacts/prompts/release/80-audit-chain-review-fixes.md`.
//!
//! Finding 2: a retained non-genesis suffix whose trusted newer anchor lies
//! past a removed tail must make every offline token command refuse with the
//! shared typed recovery error; it must never finish and never lower either
//! checkpoint store.
//!
//! Finding 4: an authenticated final record that lost only its LF must make
//! `token create` and `token list` refuse with the same typed
//! `UnterminatedActiveRecord` verdict boot gives, preserve the exact file
//! bytes, and never deliver a secret or append a record.
//!
//! Every fixture uses the real persistent stores (server_meta, users, token
//! store, audit checkpoint keyspace, audit.log) built through the CLI itself.

use super::super::{
    args::{CreateArgs, ListArgs},
    run_with_io, CliIo, TokenAction,
};
use crate::audit_appender::{AuditRecoveryError, FjallAuditAppender};
use crate::config::Config;
use crate::engine::server_meta::ServerMetaStore;
use crate::security::tokens::TokenStore;
use crate::security::user_directory::FjallUserDirectory;
use anyhow::Error as AnyError;
use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::admin::UserDirectory as _;
use shamir_connect::server::audit_chain::AuditChain;
use shamir_connect::server::user_record::UserRecord;
use tempfile::TempDir;
use zeroize::Zeroizing;

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

fn create_args(user: &str, label: &str) -> TokenAction {
    TokenAction::Create(CreateArgs {
        user: user.into(),
        label: label.into(),
        ttl: Some(7200),
        read_write: false,
        channels: "both".into(),
        databases: vec![],
        out_file: None,
    })
}

fn list_args() -> TokenAction {
    TokenAction::List(ListArgs { user: None })
}

async fn run(
    c: &Config,
    a: TokenAction,
    terminal: bool,
) -> (Result<(), AnyError>, Vec<u8>, Vec<u8>) {
    let mut o = Vec::new();
    let mut e = Vec::new();
    let mut io = streams(&mut o, &mut e, terminal);
    let result = run_with_io(c, a, &mut io).await;
    (result, o, e)
}

/// Creates one token through the real CLI and checks the secret is delivered.
async fn create_ok(c: &Config, label: &str) {
    let (result, o, _e) = run(c, create_args("alice", label), true).await;
    result.expect("healthy create must succeed");
    let text = String::from_utf8(o).expect("stdout");
    assert!(text.lines().next().unwrap_or_default().starts_with("shk1."));
}

/// Builds a genesis-containing healthy history of `labels.len()` records via
/// real CLI runs and finishes with a `token list`, whose checkpoint write
/// makes BOTH anchor stores trust the position past the final record.
async fn seeded_dir(labels: &[&str]) -> (TempDir, Config) {
    let tmp = TempDir::new().expect("tmp");
    let c = config(&tmp);
    std::fs::create_dir_all(&c.data_dir).expect("data dir");
    let dir = FjallUserDirectory::open(c.data_dir.join("users")).expect("directory");
    dir.insert("alice".into(), user_record()).expect("user");
    drop(dir);
    for label in labels {
        create_ok(&c, label).await;
    }
    let (result, _o, _e) = run(&c, list_args(), false).await;
    result.expect("healthy list must succeed");
    (tmp, c)
}

/// Walks the anyhow chain for the shared typed audit recovery error, so the
/// assertion is independent of whether the caller wraps it further.
fn recovery_error(err: &AnyError) -> &AuditRecoveryError {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<AuditRecoveryError>())
        .expect("command must fail with the shared typed audit recovery error")
}

fn log_path(c: &Config) -> std::path::PathBuf {
    c.data_dir.join("audit.log")
}

fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .collect()
}

fn write_log(c: &Config, bytes: &[u8]) {
    std::fs::write(log_path(c), bytes).expect("rewrite audit.log");
}

fn appender_anchor(c: &Config) -> Option<(u64, [u8; 32])> {
    FjallAuditAppender::load_checkpoint(&c.data_dir).expect("load appender checkpoint")
}

fn meta_anchor(c: &Config) -> Option<(u64, [u8; 32])> {
    let meta =
        ServerMetaStore::open_or_init(c.data_dir.join("server_meta")).expect("reopen server_meta");
    let cp = meta.audit_checkpoint();
    drop(meta);
    cp
}

async fn token_rows(c: &Config) -> usize {
    TokenStore::open(&c.data_dir)
        .await
        .expect("token store")
        .scan()
        .await
        .expect("scan tokens")
        .len()
}

#[tokio::test]
async fn token_list_refuses_retained_suffix_with_trusted_anchor_past_removed_tail() {
    let (_tmp, c) = seeded_dir(&["one", "two", "three"]).await;
    let bytes = std::fs::read(log_path(&c)).expect("read audit.log");
    let lines = split_lines(&bytes);
    assert_eq!(lines.len(), 3, "three creates must leave three records");

    // The retained suffix starts at the second record: it is non-genesis and
    // authenticates on its own, which is exactly the supported retention case.
    let mut suffix = lines[1].to_vec();
    suffix.push(b'\n');
    write_log(&c, &suffix);
    let entries = FjallAuditAppender::read_log_for_verify(&c.data_dir).expect("suffix parses");
    assert_eq!(entries.len(), 1);
    assert!(
        entries[0].seq > 1,
        "retained suffix must not start at genesis"
    );
    let anchor = appender_anchor(&c);
    let trusted = anchor.expect("token list must have left a trusted appender anchor");
    assert_eq!(
        trusted.0, 4,
        "trusted anchor must sit past the removed tail"
    );
    assert_eq!(meta_anchor(&c), anchor, "both stores must agree");

    let (result, o, _e) = run(&c, list_args(), false).await;
    let err = result.expect_err("token list must refuse an anchor past the retained tail");
    assert!(
        matches!(
            recovery_error(&err),
            AuditRecoveryError::AnchorPastTail {
                next_seq: 4,
                verified_next_seq: 3,
            }
        ),
        "expected AnchorPastTail, got {err:?}"
    );
    assert!(o.is_empty(), "no listing may be produced: {o:?}");

    // Refusal is side-effect free: log bytes and BOTH anchors are unchanged.
    assert_eq!(
        std::fs::read(log_path(&c)).expect("reread audit.log"),
        suffix,
        "refused token list must not rewrite the log"
    );
    assert_eq!(
        appender_anchor(&c),
        Some(trusted),
        "the appender anchor must never be lowered by the CLI"
    );
    assert_eq!(
        meta_anchor(&c),
        Some(trusted),
        "the meta anchor must never be lowered by the CLI"
    );
}

#[tokio::test]
async fn token_list_accepts_healthy_retained_suffix_anchored_at_its_tail() {
    let (_tmp, c) = seeded_dir(&["one", "two", "three"]).await;
    let bytes = std::fs::read(log_path(&c)).expect("read audit.log");
    let lines = split_lines(&bytes);
    // Sanity control on the COMPLETE history before pruning: it verifies from
    // genesis with the persisted key.
    let key = {
        let meta = ServerMetaStore::open_or_init(c.data_dir.join("server_meta"))
            .expect("open server_meta");
        let key = meta.audit_chain_key().expect("audit chain key");
        drop(meta);
        key
    };
    let full = FjallAuditAppender::read_log_for_verify(&c.data_dir).expect("full log parses");
    assert_eq!(full.len(), 3);
    AuditChain::verify_chain(&key, &full).expect("complete fixture history must authenticate");

    let mut suffix = lines[1].to_vec();
    suffix.push(b'\n');
    suffix.extend_from_slice(lines[2]);
    suffix.push(b'\n');
    let anchor_before = appender_anchor(&c);
    let meta_before = meta_anchor(&c);
    write_log(&c, &suffix);
    // The retained suffix parses on its own; verify_chain from genesis is NOT
    // expected to accept a non-genesis suffix, that refusal is the point of
    // the other regression.
    let entries = FjallAuditAppender::read_log_for_verify(&c.data_dir).expect("suffix parses");
    assert_eq!(entries.len(), 2);
    assert!(entries[0].seq > 1, "control suffix must start past genesis");

    let (result, o, _e) = run(&c, list_args(), false).await;
    result.expect("token list must work on a healthy retained suffix");
    let text = String::from_utf8(o).expect("listing");
    assert!(text.contains("two"));
    assert!(text.contains("three"));
    assert_eq!(
        std::fs::read(log_path(&c)).expect("reread audit.log"),
        suffix,
        "token list appends nothing"
    );
    assert_eq!(
        appender_anchor(&c),
        anchor_before,
        "healthy list must keep the trusted anchors"
    );
    assert_eq!(meta_anchor(&c), meta_before);
}

#[tokio::test]
async fn create_and_list_refuse_unterminated_final_record_and_never_append() {
    let (_tmp, c) = seeded_dir(&["seed", "control"]).await;
    let bytes = std::fs::read(log_path(&c)).expect("read audit.log");
    assert!(
        bytes.ends_with(b"\n"),
        "fixture sanity: history must end with LF"
    );
    assert_eq!(split_lines(&bytes).len(), 2);
    let stripped = &bytes[..bytes.len() - 1];
    // The typed offset is the unterminated record's BEGINNING, not its end.
    let record_start = stripped
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0usize, |pos| pos + 1);
    write_log(&c, stripped);
    assert_eq!(token_rows(&c).await, 2, "fixture sanity: two tokens");

    // `token create` must give boot's verdict: typed UnterminatedActiveRecord,
    // no secret delivered on either stream, no append, no new token row.
    let (result, o, e) = run(&c, create_args("alice", "after"), true).await;
    let err = result.expect_err("create must refuse an unterminated final record");
    assert!(
        matches!(
            recovery_error(&err),
            AuditRecoveryError::UnterminatedActiveRecord { offset, .. }
            if *offset as usize == record_start
        ),
        "expected UnterminatedActiveRecord at offset {record_start}, got {err:?}"
    );
    let stdout = String::from_utf8(o).expect("stdout");
    let stderr = String::from_utf8(e).expect("stderr");
    assert!(
        !stdout.contains("shk1."),
        "no token secret may be delivered on stdout: {stdout:?}"
    );
    assert!(
        !stderr.contains("shk1."),
        "no token secret may be delivered on stderr: {stderr:?}"
    );
    assert_eq!(
        std::fs::read(log_path(&c)).expect("reread audit.log"),
        stripped,
        "refused create must not append to the unterminated record"
    );
    assert_eq!(
        token_rows(&c).await,
        2,
        "refused create must leave no token row behind"
    );

    // `token list` refuses with the SAME typed verdict and stays read-only.
    let (result, o, _e) = run(&c, list_args(), false).await;
    let err = result.expect_err("list must refuse an unterminated final record");
    assert!(
        matches!(
            recovery_error(&err),
            AuditRecoveryError::UnterminatedActiveRecord { offset, .. }
            if *offset as usize == record_start
        ),
        "list must agree with create and boot, got {err:?}"
    );
    assert!(o.is_empty(), "no listing may be produced");
    assert_eq!(
        std::fs::read(log_path(&c)).expect("reread audit.log"),
        stripped,
        "refused list must not rewrite the log"
    );
}

/// Sanity control on the real fixture: the pristine directory keeps working
/// across repeated offline commands after its last clean close.
#[tokio::test]
async fn healthy_history_still_works_across_repeated_cli_reopens() {
    let tmp = TempDir::new().expect("tmp");
    let c = config(&tmp);
    std::fs::create_dir_all(&c.data_dir).expect("data dir");
    let dir = FjallUserDirectory::open(c.data_dir.join("users")).expect("directory");
    dir.insert("alice".into(), user_record()).expect("user");
    drop(dir);
    create_ok(&c, "first").await;
    let before = std::fs::read(log_path(&c)).expect("read audit.log");
    for cycle in 1..=2 {
        let (result, o, _e) = run(&c, list_args(), false).await;
        result.unwrap_or_else(|err| panic!("healthy list cycle {cycle} refused: {err:?}"));
        assert!(String::from_utf8(o).expect("listing").contains("first"));
    }
    assert_eq!(
        std::fs::read(log_path(&c)).expect("reread audit.log"),
        before,
        "repeated healthy lists must not touch the log"
    );
}
