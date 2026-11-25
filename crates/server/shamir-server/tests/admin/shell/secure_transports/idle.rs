//! Review item 1: an interactive WSS SCRAM session must survive server
//! heartbeats while the shell sits at the prompt. The scripted REPL runs in
//! a real child (real stdin/stderr/stdout pipes, human output on stderr);
//! the parent writes the secret line FIRST (zeroizing buffer), then proves
//! the FIRST ping outcome was written, then holds the stdin pipe open
//! (blocking read, no EOF) for longer than the real heartbeat tunables
//! (`WS_PING_INTERVAL` + `WS_PONG_TIMEOUT`), and only then sends a second
//! `ping` that must still succeed. Without the parser fix the SDK reader
//! task is suspended between commands, the server closes the session (4003)
//! and the second ping fails: this test is red there.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use zeroize::Zeroizing;

use shamir_tunables::instance_defaults::{WS_PING_INTERVAL, WS_PONG_TIMEOUT};

use crate::shell_tests::harness::{ShellRun, DB_NAME};
use crate::shell_tests::secure_fixture::{
    boot_secure, scan_operator_home, scripted_child_command, secret_suffix, write_secure_profile,
};

use super::capture::{assert_clean, CHILD_ENTRY, CHILD_GATE};

/// Generous hang bound for the whole probe (idle window included).
const TOTAL_BOUND: Duration = Duration::from_secs(240);

/// Per-phase read bound: an elapse means the child hung, never a slow
/// runner.
const READ_BOUND: Duration = Duration::from_secs(120);

#[test]
fn shell_secure_wss_scram_idle_heartbeat_survives() {
    let fixture = boot_secure("shell_secure_wss_scram_idle_heartbeat_survives");
    let shell = &fixture.shell;
    write_secure_profile(shell, "wss", fixture.wss_addr, DB_NAME, None);
    let password = shell.canary.clone();
    let suffix = secret_suffix(&password).to_owned();
    let idle_for = WS_PING_INTERVAL + WS_PONG_TIMEOUT + Duration::from_secs(10);

    shell.guard.mark("idle: spawn scripted child");
    shell.guard.note("idle_for", format!("{idle_for:?}"));
    // Assembled OUTSIDE the runtime closure so `password` stays available
    // below and the zeroizing line is the only secret copy that moves in.
    let mut secret_line = Zeroizing::new(Vec::with_capacity(password.len() + 1));
    secret_line.extend_from_slice(password.as_bytes());
    secret_line.push(b'\n');
    let mut command = scripted_child_command(shell, CHILD_GATE, CHILD_ENTRY);
    let outcome = shell.rt.run(bound(TOTAL_BOUND, async move {
        let mut child = command.spawn().expect("spawn scripted child");
        let mut stdin = child.stdin.take().expect("child stdin");
        let mut stderr = BufReader::new(child.stderr.take().expect("child stderr"));
        let mut stdout = child.stdout.take().expect("child stdout");

        // The child entry consumes the FIRST stdin line as the secret;
        // only then do the command lines follow.
        stdin
            .write_all(secret_line.as_slice())
            .await
            .expect("write secret line");
        stdin.flush().await.expect("flush secret line");
        write_line(&mut stdin, "connect\n").await;
        write_line(&mut stdin, "ping\n").await;
        // Raw stderr lines, byte-exact — never lossy-reconstructed text.
        let mut lines: Vec<Vec<u8>> = Vec::new();
        // The first ping outcome is PROVEN written before the idle window
        // starts; the stdin pipe stays open, so the child blocks in its
        // real bounded read for the whole idle.
        expect_outcome(&mut stderr, b"connect", &mut lines).await;
        expect_outcome(&mut stderr, b"ping", &mut lines).await;

        tokio::time::sleep(idle_for).await;

        // No premature teardown: the same session must still round-trip.
        write_line(&mut stdin, "ping\n").await;
        write_line(&mut stdin, "quit\n").await;
        drop(stdin);
        let mut tail = Vec::new();
        stderr
            .read_to_end(&mut tail)
            .await
            .expect("read child stderr to end");
        split_lines(&tail, &mut lines);
        let mut stdout_bytes = Vec::new();
        stdout
            .read_to_end(&mut stdout_bytes)
            .await
            .expect("read child stdout to end");
        let code = child
            .wait()
            .await
            .expect("scripted child waits")
            .code()
            .unwrap_or(-1);
        (code, lines, stdout_bytes)
    }));
    let (code, lines, stdout_bytes) = outcome;
    shell.guard.note("idle child exit", code.to_string());

    // The child asserts its own shell exit; a failing child returns the
    // libtest failure code, which this assertion propagates.
    assert_eq!(code, 0, "scripted child exited {code}");
    // ALL captured bytes are scanned — the child's stdout legitimately
    // carries libtest framing ("running 1 test", "test result: ok"); the
    // invariant is that the SHELL adds nothing, in particular no secret.
    let run = ShellRun {
        exit_code: code,
        stdout: stdout_bytes,
        stderr: lines.concat(),
    };
    assert_clean(&run, password.as_str());
    let outcomes: Vec<&[u8]> = lines
        .iter()
        .map(|line| trim_eol(line))
        .filter(|line| matches!(*line, b"connect" | b"ping" | b"error"))
        .collect();
    assert!(
        outcomes
            == [
                b"connect".as_slice(),
                b"ping".as_slice(),
                b"ping".as_slice()
            ],
        "expected connect, pre-idle ping, and a successful post-idle ping"
    );
    scan_operator_home(shell, &[password.as_str(), &suffix]);
}

/// Strips one trailing LF and an immediately preceding CR (CRLF on
/// Windows children), byte-exact.
fn trim_eol(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn split_lines(bytes: &[u8], lines: &mut Vec<Vec<u8>>) {
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(bytes[start..=index].to_vec());
            start = index + 1;
        }
    }
    if start < bytes.len() {
        lines.push(bytes[start..].to_vec());
    }
}

async fn write_line(stdin: &mut tokio::process::ChildStdin, line: &str) {
    stdin
        .write_all(line.as_bytes())
        .await
        .expect("write scripted line");
    stdin.flush().await.expect("flush scripted line");
}

/// Reads child stderr lines (raw bytes via `read_until`) until the outcome
/// head `head` is written. Human rendering writes the command id (or
/// `error`) as its own first line, so a byte-exact line match is exact.
async fn expect_outcome(
    stderr: &mut BufReader<tokio::process::ChildStderr>,
    head: &[u8],
    lines: &mut Vec<Vec<u8>>,
) {
    loop {
        let mut buf = Vec::new();
        let read = tokio::time::timeout(READ_BOUND, stderr.read_until(b'\n', &mut buf))
            .await
            .expect("child outcome within read bound")
            .expect("read child stderr");
        assert!(read > 0, "child stderr ended before {} bytes", head.len());
        let matched = trim_eol(&buf) == head;
        lines.push(buf);
        if matched {
            return;
        }
    }
}

/// Absolute-bound wrapper (same discipline as the harness `bound`): an
/// elapse means a genuine hang.
async fn bound<T>(limit: Duration, future: impl std::future::Future<Output = T>) -> T {
    match tokio::time::timeout(limit, future).await {
        Ok(value) => value,
        Err(_) => panic!("idle probe exceeded its {limit:?} hang bound"),
    }
}
