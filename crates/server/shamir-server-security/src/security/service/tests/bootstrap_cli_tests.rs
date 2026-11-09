use std::io::{BufReader, Cursor};
use std::path::PathBuf;

use clap::{error::ErrorKind, Parser};
use zeroize::Zeroizing;

use super::{bootstrap_argv_warning_needed, bootstrap_mode, bootstrap_stdin, BootstrapMode, Cli};

fn read(bytes: &[u8]) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    bootstrap_stdin::read_password(BufReader::with_capacity(2, Cursor::new(bytes)))
}

#[test]
fn rv_server_97c240d7_line_endings_and_whitespace() {
    for (input, expected) in [
        (&b"  pass \t\nignored"[..], &b"  pass \t"[..]),
        (&b" pass \r\nignored"[..], &b" pass "[..]),
        (&b" pass \r"[..], &b" pass \r"[..]),
        (&b"pass"[..], &b"pass"[..]),
        (&b"\r"[..], &b"\r"[..]),
        (&b" \n"[..], &b" "[..]),
    ] {
        assert_eq!(&*read(input).expect("valid line"), expected);
    }
}

#[test]
fn rv_server_97c240d7_empty_eof_invalid_and_size_boundaries() {
    for input in [&b""[..], &b"\n"[..], &b"\r\n"[..], &b"secret\xff\n"[..]] {
        let error = read(input).expect_err("invalid input").to_string();
        assert!(!error.contains("secret"));
    }
    let max = vec![b'x'; 4096];
    assert_eq!(read(&max).unwrap().len(), 4096);
    let mut crlf = max.clone();
    crlf.extend_from_slice(b"\r\n");
    assert_eq!(read(&crlf).unwrap().len(), 4096);
    let mut oversized = max;
    oversized.push(b'x');
    assert!(read(&oversized).is_err());
    oversized.extend_from_slice(b"\n");
    assert!(read(&oversized).is_err());
}

#[test]
fn rv_server_97c240d7_clap_conflicts_and_help() {
    for args in [
        vec![
            "server",
            "--config",
            "cfg",
            "--bootstrap-password-stdin",
            "--bootstrap-password",
            "secret",
        ],
        vec![
            "server",
            "--config",
            "cfg",
            "--bootstrap-password-stdin",
            "--skip-bootstrap",
        ],
    ] {
        let error = Cli::try_parse_from(args).err().expect("conflict");
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
        assert!(!error.to_string().contains("secret"));
    }
    let help = Cli::try_parse_from(["server", "--help"])
        .err()
        .unwrap()
        .to_string();
    assert!(help.contains("process lists"));
    assert!(help.contains("shell history"));
}

#[test]
fn rv_server_97c240d7_existing_modes_and_stdin_selection() {
    assert!(bootstrap_argv_warning_needed(&Some("sensitive".into())));
    assert!(!bootstrap_argv_warning_needed(&None));
    let cli = Cli::try_parse_from(["server", "--config", "cfg"]).unwrap();
    assert!(!cli.bootstrap_password_stdin);
    assert!(matches!(
        bootstrap_mode(false, None, None, None, None),
        BootstrapMode::RandomToken {
            username: None,
            token_path: None
        }
    ));
    assert!(matches!(
        bootstrap_mode(true, None, None, None, None),
        BootstrapMode::Skip
    ));
    let path = PathBuf::from("token");
    assert!(
        matches!(bootstrap_mode(false, None, None, Some("user".into()), Some(path.clone())), BootstrapMode::RandomToken { username: Some(user), token_path: Some(actual) } if user == "user" && actual == path)
    );
    for (argv, stdin, expected) in [
        (
            Some("argv password".to_string()),
            None,
            &b"argv password"[..],
        ),
        (
            None,
            Some(Zeroizing::new(b"stdin password".to_vec())),
            &b"stdin password"[..],
        ),
    ] {
        match bootstrap_mode(false, argv, stdin, None, None) {
            BootstrapMode::Password { username, password } => {
                assert_eq!(username, shamir_server::bootstrap::DEFAULT_BOOTSTRAP_NAME);
                assert_eq!(&*password, expected);
            }
            _ => panic!("expected password mode"),
        }
    }
}
