use crate::{supervise, ProcessTree, TeardownFaults};
use std::io::{BufRead, BufReader, Read};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

fn bounded<F: FnOnce() + Send + 'static>(f: F) {
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(())) => {}
        Ok(Err(payload)) => std::panic::resume_unwind(payload),
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("teardown hung"),
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("teardown worker disconnected"),
    }
    let _ = worker.join();
}

fn command(script: &str) -> Command {
    #[cfg(unix)]
    {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        use std::os::windows::process::CommandExt;
        if script == straggler_command() {
            command.raw_arg(r#"/C start "" /b cmd /C "echo ready& ping -n 1000 127.0.0.1 >NUL""#);
        } else {
            command.raw_arg(format!(r#"/C cmd /C "{script}""#));
        }
        command
    }
}
fn timeout_command() -> &'static str {
    #[cfg(unix)]
    {
        "sleep 1000 & echo ready; wait"
    }
    #[cfg(windows)]
    {
        r#"echo ready& ping -n 1000 127.0.0.1 >NUL"#
    }
}
fn straggler_command() -> &'static str {
    #[cfg(unix)]
    {
        "sleep 1000 & echo ready"
    }
    #[cfg(windows)]
    {
        r#"start "" /b cmd /C "echo ready& ping -n 1000 127.0.0.1 >NUL""#
    }
}
fn ready(
    script: &str,
    timeout: Duration,
    faults: TeardownFaults,
) -> Result<std::process::Output, String> {
    let mut cmd = command(script);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (child, tree) = ProcessTree::spawn(&mut cmd).expect("spawn");
    let mut child = child;
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut line = String::new();
    loop {
        line.clear();
        if stdout.read_line(&mut line).expect("read readiness") == 0 {
            panic!("process exited before readiness handshake");
        }
        if line.trim() == "ready" {
            break;
        }
    }
    let stderr = child.stderr.take();
    supervise(child, tree, Some(stdout), stderr, timeout, faults)
}

#[test]
fn rv_wasm_host_58c052b0_timeout_kills_descendants_holding_pipes() {
    bounded(|| {
        let error =
            ready(timeout_command(), Duration::ZERO, TeardownFaults::default()).unwrap_err();
        assert!(error.contains("compilation timed out"), "{error}");
        assert!(!error.contains("teardown incomplete"), "{error}");
    });
}
#[test]
fn rv_wasm_host_58c052b0_wait_error_kills_tree() {
    bounded(|| {
        let error = ready(
            timeout_command(),
            Duration::from_secs(600),
            TeardownFaults {
                fail_wait: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            error.contains("cargo wait failed: injected wait failure"),
            "{error}"
        );
        assert!(!error.contains("teardown incomplete"), "{error}");
    });
}
#[test]
fn rv_wasm_host_58c052b0_kill_failure_is_bounded() {
    bounded(|| {
        let error = ready(
            timeout_command(),
            Duration::ZERO,
            TeardownFaults {
                fail_tree_kill: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            error.contains("tree kill: injected tree kill failure"),
            "{error}"
        );
        assert!(error.contains("teardown incomplete"), "{error}");
        assert!(error.contains("drain timed out"), "{error}");
    });
}
#[test]
fn rv_wasm_host_58c052b0_exited_leader_straggler_is_torn_down() {
    bounded(|| {
        let output = ready(
            straggler_command(),
            Duration::from_secs(10),
            TeardownFaults::default(),
        )
        .expect("supervision");
        assert!(output.status.success());
    });
}
#[test]
fn rv_wasm_host_58c052b0_dropping_tree_kills_descendants() {
    bounded(|| {
        let mut cmd = command(timeout_command());
        cmd.stdout(std::process::Stdio::piped());
        let (mut child, tree) = ProcessTree::spawn(&mut cmd).expect("spawn");
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut line = String::new();
        if stdout.read_line(&mut line).expect("read readiness") == 0 {
            panic!("process exited before readiness handshake");
        }
        assert_eq!(line.trim(), "ready");
        drop(tree);
        let mut remaining = Vec::new();
        stdout.read_to_end(&mut remaining).expect("read to EOF");
        child.wait().expect("wait");
    });
}
