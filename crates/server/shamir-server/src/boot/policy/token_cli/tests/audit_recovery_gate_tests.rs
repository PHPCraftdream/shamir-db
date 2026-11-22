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
//! Round 3 adds lock-first, output-alias and lock-error-classification
//! regressions for `83-audit-chain-round3.md` finding 3.

use super::super::{
    args::{CreateArgs, ListArgs},
    lock::{is_lock_contention, LockFailure},
    run_with_io,
    secret_output::OutputPathError,
    CliIo, TokenAction,
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
#[cfg(unix)]
use std::os::unix::fs::symlink;
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

fn create_args_with_out(user: &str, label: &str, out: &std::path::Path) -> TokenAction {
    TokenAction::Create(CreateArgs {
        user: user.into(),
        label: label.into(),
        ttl: Some(7200),
        read_write: false,
        channels: "both".into(),
        databases: vec![],
        out_file: Some(out.to_path_buf()),
    })
}

fn typed_output_error(err: &AnyError, what: &str) {
    assert!(
        err.chain()
            .any(|cause| cause.downcast_ref::<OutputPathError>().is_some()),
        "{what} must fail with the typed output-path error, got {err:#}"
    );
}

/// Round-3 finding 3: `--out-file` equal to the MISSING active audit log.
/// The valid archive stays, the active log stays missing (recovery may
/// recreate an empty active only AFTER the typed refusal), and no secret is
/// ever written into the audit namespace. Old behavior failed here because
/// `prepare` created the missing `audit.log` BEFORE the lock and recovery:
/// recovery then accepted the empty active file over the valid archive and
/// the created token appended and delivered its secret into `audit.log`.
#[tokio::test]
async fn create_refuses_audit_log_output_when_active_is_missing_and_archive_is_valid() {
    let (_tmp, c) = seeded_dir(&["one", "two"]).await;
    let bytes = std::fs::read(log_path(&c)).expect("read audit.log");
    let lines = split_lines(&bytes);
    assert_eq!(lines.len(), 2, "fixture sanity: two records");
    let key = {
        let meta = ServerMetaStore::open_or_init(c.data_dir.join("server_meta"))
            .expect("open server_meta");
        let key = meta.audit_chain_key().expect("audit chain key");
        drop(meta);
        key
    };
    let entries = FjallAuditAppender::read_log_for_verify(&c.data_dir).expect("log parses");
    AuditChain::verify_chain(&key, &entries).expect("fixture history must authenticate");
    // Simulate an interrupted rotation: valid archive, no active file.
    let archive = c.data_dir.join("audit.log.00000000000000009999");
    std::fs::rename(log_path(&c), &archive).expect("rename active to archive");
    assert!(!log_path(&c).exists(), "fixture sanity: active is missing");

    let (result, o, e) = run(
        &c,
        create_args_with_out("alice", "after", &log_path(&c)),
        true,
    )
    .await;
    let err = result.expect_err("create must refuse an output path equal to the audit log");
    typed_output_error(&err, "create");
    let stdout = String::from_utf8(o).expect("stdout");
    assert!(!stdout.contains("shk1."), "no secret on stdout: {stdout:?}");
    assert!(
        !String::from_utf8(e).expect("stderr").contains("shk1."),
        "no secret on stderr"
    );
    assert!(
        !log_path(&c).exists(),
        "the typed refusal must not recreate the missing active log"
    );
    assert_eq!(
        std::fs::read(&archive).expect("archive still present"),
        bytes,
        "the valid archive must be byte-identical after the refusal"
    );
}

/// Round-3 regression: a healthy `--out-file` create delivers the secret at
/// exactly the requested path. A miscomputed validated destination once
/// pointed below the data directory, so every file-delivered create failed.
#[tokio::test]
async fn create_with_out_file_delivers_the_secret_at_the_requested_path() {
    let (tmp, c) = seeded_dir(&["one"]).await;
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside dir");
    let out = outside.join("secret.out");
    let (result, o, _e) = run(&c, create_args_with_out("alice", "to-file", &out), true).await;
    result.expect("create with a fresh out file must succeed");
    let secret = std::fs::read_to_string(&out).expect("secret file at the requested path");
    assert!(
        secret.starts_with("shk1."),
        "the secret must be delivered to the file: {secret:?}"
    );
    assert!(
        !String::from_utf8(o)
            .expect("stdout")
            .contains(secret.trim_end()),
        "the secret must not also reach stdout"
    );
}

/// Windows only: a final component such as `z:notes` would be re-parsed as a
/// drive prefix and replace the resolved parent (and ':' is an NTFS stream
/// separator), so it is refused with the typed error and nothing is written.
#[cfg(windows)]
#[tokio::test]
async fn create_refuses_a_windows_drive_like_final_component() {
    let (tmp, c) = seeded_dir(&["one"]).await;
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside dir");
    let out = std::path::PathBuf::from(format!("{}\\z:notes", outside.display()));
    let (result, o, _e) = run(&c, create_args_with_out("alice", "ads", &out), true).await;
    let err = result.expect_err("a ':' component must be refused");
    typed_output_error(&err, "create");
    assert!(!String::from_utf8(o).expect("stdout").contains("shk1."));
}

/// Round-3 finding 3: with a REAL held maintenance lock, the refusal must
/// come before any output creation. The strong oracle is the ALREADY
/// EXISTING destination: `prepare` would fail it with an exclusive-create
/// error, so a lock message proves the lock won first. Old behavior failed
/// the occupied case outright (it reported the output error) and it
/// mislabeled every lock failure as a plain string with no typed split.
#[tokio::test]
async fn held_lock_refuses_create_before_any_output_file_is_created() {
    let (_tmp, c) = seeded_dir(&["one"]).await;
    let log_before = std::fs::read(log_path(&c)).expect("read audit.log");
    let _guard = crate::maintenance_lock::acquire(&c.data_dir).expect("hold the lock");

    let occupied = c.data_dir.join("occupied.out");
    std::fs::write(&occupied, b"keep").expect("existing destination");
    let (result, _o, _e) = run(&c, create_args_with_out("alice", "held", &occupied), true).await;
    let err = result.expect_err("a held lock must refuse create");
    let text = format!("{err:#}");
    assert!(
        text.contains("requires a stopped server"),
        "contention must be reported as such: {text}"
    );
    assert!(
        !text.contains("exclusively create"),
        "the lock refusal must win before output creation: {text}"
    );
    assert_eq!(
        std::fs::read(&occupied).expect("occupied file intact"),
        b"keep",
        "an existing destination must never be clobbered"
    );

    let fresh = c.data_dir.join("fresh.out");
    let (result, _o, _e) = run(&c, create_args_with_out("alice", "held", &fresh), true).await;
    let err = result.expect_err("a held lock must refuse a fresh destination too");
    assert!(
        format!("{err:#}").contains("requires a stopped server"),
        "got {err:#}"
    );
    assert!(!fresh.exists(), "a lock refusal must not create the output");
    assert_eq!(
        std::fs::read(log_path(&c)).expect("reread audit.log"),
        log_before,
        "a lock refusal must not touch the audit log"
    );
}

/// Round-3 finding 3: permission failures at the lock are classified
/// separately from contention and keep the IO error detail. Deterministic
/// via the production classifier and `fs4`'s own contended-error oracle;
/// real filesystem permission behavior differs between CI privilege levels.
#[test]
fn lock_error_classification_distinguishes_permission_from_contention() {
    let contended = fs4::lock_contended_error();
    assert!(
        is_lock_contention(&contended),
        "fs4's own contended error must classify as contention"
    );
    assert!(
        !is_lock_contention(&std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        "permission denial is not contention"
    );
    // Raw os error 5 is a denial on Windows; on Unix it is a different
    // non-contention error, so either way it must not be contention.
    assert!(
        !is_lock_contention(&std::io::Error::from_raw_os_error(5)),
        "arbitrary raw errors are not contention"
    );
    assert_eq!(
        LockFailure::Contention.to_string(),
        "offline token command requires a stopped server"
    );
    let denied = LockFailure::LockOther(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    let text = denied.to_string();
    assert!(
        !text.contains("requires a stopped server"),
        "a permission error must not read as contention: {text}"
    );
    assert!(
        text.contains("denied"),
        "the IO detail must survive: {text}"
    );
    let open_failed = LockFailure::Open(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    assert!(
        open_failed.to_string().contains("cannot open"),
        "an open failure must say so: {open_failed}"
    );
}

/// Round-3 finding 3: every alias into the reserved `audit.log*` namespace
/// is refused with the typed error and the real audit log is untouched.
/// Old behavior created or failed on the alias itself instead of typing the
/// refusal, and the dotdot and reserved-directory descendants were not
/// checked at all.
#[tokio::test]
async fn create_refuses_every_reserved_audit_namespace_alias() {
    let (_tmp, c) = seeded_dir(&["one"]).await;
    let before = std::fs::read(log_path(&c)).expect("read audit.log");
    std::fs::create_dir_all(c.data_dir.join("audit.log.dir").join("grand"))
        .expect("reserved-named directory tree");
    std::fs::create_dir_all(c.data_dir.join("sub")).expect("dotdot base directory");
    // ASCII case variants are reserved on EVERY platform (Windows and macOS
    // defaults are case-insensitive; Linux can mount case-insensitive
    // filesystems), so the uppercase archive shape is refused everywhere.
    // No entry with that spelling exists in the fixture.
    let aliases: Vec<std::path::PathBuf> = vec![
        log_path(&c),
        c.data_dir.join("audit.log.00000000000000000042"),
        c.data_dir.join("AUDIT.LOG.00000000000000000042"),
        c.data_dir
            .join("audit.log.dir")
            .join("grand")
            .join("secret.txt"),
        c.data_dir.join("sub").join("..").join("audit.log"),
    ];
    for alias in &aliases {
        let (result, _o, _e) = run(&c, create_args_with_out("alice", "alias", alias), true).await;
        let err = result.expect_err("a reserved alias must be refused");
        typed_output_error(&err, "create");
    }
    assert!(
        !c.data_dir.join("AUDIT.LOG.00000000000000000042").exists(),
        "the refusal must not create a case-variant archive name"
    );
    assert_eq!(
        std::fs::read(log_path(&c)).expect("reread audit.log"),
        before,
        "no alias attempt may touch the audit log"
    );
    let archive_alias = c.data_dir.join("audit.log.00000000000000000042");
    assert!(
        !archive_alias.exists(),
        "the refusal must not create an archive-named file"
    );
}

/// Round-3 follow-up: final symlink aliases are typed-refused without being
/// followed for creation. Unix only: symlink creation needs privileges on
/// Windows CI. A dangling link with a harmless name whose target is a
/// missing reserved name is refused through its bounded target check; a
/// reserved final name is refused even when the link points OUT of the data
/// directory. Old behavior reported the plain exclusive-create error
/// instead of the typed refusal and never inspected the target at all.
#[cfg(unix)]
#[tokio::test]
async fn create_typed_refuses_symlink_aliases_named_or_pointing_into_audit_namespace() {
    let (_tmp, c) = seeded_dir(&["one"]).await;
    let before = std::fs::read(log_path(&c)).expect("read audit.log");
    // Dangling link with a harmless name into a missing protected name.
    let into = c.data_dir.join("secret.out");
    symlink(c.data_dir.join("audit.log.00000000000000000042"), &into)
        .expect("dangling link into the reserved namespace");
    // Reserved final name pointing OUT of the data directory.
    let outside = c
        .data_dir
        .parent()
        .unwrap_or(c.data_dir.as_path())
        .join("outside.txt");
    std::fs::write(&outside, b"out").expect("outside file");
    let named = c.data_dir.join("audit.log.00000000000000000077");
    symlink(&outside, &named).expect("reserved name pointing out");
    for alias in [&into, &named] {
        let (result, _o, _e) = run(&c, create_args_with_out("alice", "alias", alias), true).await;
        let err = result.expect_err("a symlink alias into the audit namespace must be refused");
        typed_output_error(&err, "create");
    }
    assert_eq!(
        std::fs::read(log_path(&c)).expect("reread audit.log"),
        before,
        "no symlink attempt may touch the audit log"
    );
    assert!(
        !c.data_dir.join("audit.log.00000000000000000042").exists(),
        "nothing may be created through the dangling link's target"
    );
}
