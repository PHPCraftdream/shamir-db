//! Format-stamp wiring for the offline token CLI (design 6.2):
//! an empty directory is stamped before `server_meta` is created.

use shamir_sync::format_stamp::{inspect, StampState, CURRENT_FORMAT_EPOCH, STAMP_FILE_NAME};

use super::super::{args::CreateArgs, run_with_io, CliIo, TokenAction};
use crate::config::Config;
use tempfile::TempDir;

fn config(root: &TempDir) -> Config {
    let src="data_dir: data\nkdf_defaults: { memory_kb: 19456, time: 2, parallelism: 1, argon2_version: 19 }\nlisteners: [{ kind: tcp, addr: 127.0.0.1:7331, profile: tls_exporter }]\ntls: { cert_path: cert.pem, key_path: key.pem }\naudit: { max_file_size_mb: 0, retention_days: 0 }\n";
    let mut c: Config = ktav::from_str(src).expect("test config");
    c.data_dir = root.path().join("data");
    c
}

fn streams<'a>(o: &'a mut Vec<u8>, e: &'a mut Vec<u8>) -> CliIo<'a> {
    CliIo {
        stdout: o,
        stderr: e,
        is_terminal: true,
    }
}

#[tokio::test]
async fn token_cli_stamps_empty_dir_before_server_meta() {
    let tmp = TempDir::new().expect("tmp");
    let c = config(&tmp);
    std::fs::create_dir_all(&c.data_dir).expect("data dir");
    let mut o = Vec::new();
    let mut e = Vec::new();
    let a = TokenAction::Create(CreateArgs {
        user: "alice".into(),
        label: "runner".into(),
        ttl: Some(7200),
        read_write: false,
        channels: "both".into(),
        databases: vec![],
        out_file: None,
    });
    {
        let mut io = streams(&mut o, &mut e);
        run_with_io(&c, a, &mut io).await.expect("create");
    }
    assert!(
        c.data_dir.join(STAMP_FILE_NAME).is_file(),
        "stamp written before server_meta"
    );
    assert!(c.data_dir.join("server_meta").exists());
    match inspect(&c.data_dir) {
        StampState::Valid(stamp) => assert_eq!(stamp.format_epoch(), CURRENT_FORMAT_EPOCH),
        other => panic!("expected a valid stamp, got {other:?}"),
    }
}
