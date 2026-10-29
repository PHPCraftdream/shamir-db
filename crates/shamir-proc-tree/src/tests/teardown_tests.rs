use crate::{supervise, ProcessTree, TeardownFaults};
#[cfg(unix)]
use rustix::process::{kill_process_group, waitid, Pid, Signal, WaitId, WaitIdOptions};
use std::io::{self, BufRead, BufReader, Read};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::Duration;

// Own the child until cleanup or transfer. On Unix the unreaped leader pins the
// process group; never signal it once another operation has reaped the leader.
struct ChildGuard {
    child: Option<Child>,
    #[cfg(unix)]
    pgid: Pid,
    tree: Option<ProcessTree>,
}

impl ChildGuard {
    #[cfg(unix)]
    fn new(child: Child) -> Self {
        let pgid = Pid::from_child(&child);
        Self {
            child: Some(child),
            pgid,
            tree: None,
        }
    }

    fn with_tree(child: Child, tree: ProcessTree) -> Self {
        Self {
            #[cfg(unix)]
            pgid: Pid::from_child(&child),
            child: Some(child),
            tree: Some(tree),
        }
    }

    fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("guard owns child")
    }

    fn take_child(&mut self) -> Child {
        self.child.take().expect("guard owns child")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        #[cfg(unix)]
        {
            // NOWAIT observes even a zombie without releasing the PGID. ECHILD
            // (or any unexpected error) means signaling would not be safe.
            let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
            if waitid(WaitId::Pid(self.pgid), options).is_ok() {
                let _ = kill_process_group(self.pgid, Signal::KILL);
                let _ = child.kill();
            }
            // A retained ProcessTree also signals on drop, so drop it while
            // the leader is still pinned. It is only retained before transfer.
            drop(self.tree.take());
        }
        #[cfg(windows)]
        {
            if let Some(tree) = self.tree.as_ref() {
                let _ = tree.terminate();
            }
            let _ = child.kill();
        }
        let _ = crate::supervise::bounded_reap(child);
    }
}

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
    let mut guard = ChildGuard::with_tree(child, tree);
    let mut stdout = BufReader::new(guard.child().stdout.take().expect("stdout"));
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
    let stderr = guard.child().stderr.take();
    // supervise consumes both owners; its unwind path is outside this guard.
    let child = guard.take_child();
    let tree = guard.tree.take().expect("guard owns tree");
    supervise(child, tree, Some(stdout), stderr, timeout, faults)
}

#[cfg(unix)]
#[test]
fn waiter_spawn_failure_reports_failed_kills_and_reap_timeout() {
    bounded(|| {
        let mut cmd = command(timeout_command());
        cmd.process_group(0).stdout(std::process::Stdio::piped());
        let child = cmd.spawn().expect("spawn");
        let mut guard = ChildGuard::new(child);
        let mut output = BufReader::new(guard.child().stdout.take().expect("stdout"));
        let mut line = String::new();
        output.read_line(&mut line).expect("read readiness");
        assert_eq!(line.trim(), "ready");
        let pgid = Pid::from_child(guard.child());
        let error = crate::process_tree_unix::start_waiter(
            guard.child(),
            || Err(io::Error::other("injected waiter spawn failure")),
            |_| {
                (
                    Err(io::Error::other("injected tree kill failure")),
                    Err(io::Error::other("injected child kill failure")),
                )
            },
        )
        .expect_err("startup must fail");
        waitid(
            WaitId::Pid(pgid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        )
        .expect("leader must remain unreaped before cleanup kill");
        kill_process_group(pgid, Signal::KILL).expect("cleanup kill");
        assert!(crate::supervise::bounded_reap(guard.child())
            .expect("cleanup reap")
            .is_some());
        let message = error.to_string();
        assert!(
            message.contains("injected waiter spawn failure"),
            "{message}"
        );
        assert!(
            message.contains("tree kill: injected tree kill failure"),
            "{message}"
        );
        assert!(
            message.contains("child kill: injected child kill failure"),
            "{message}"
        );
        assert!(message.contains("leader could not be reaped"), "{message}");
    });
}

#[cfg(unix)]
#[test]
fn waiter_error_still_reaps_leader() {
    bounded(|| {
        let mut cmd = command(timeout_command());
        cmd.process_group(0).stdout(std::process::Stdio::piped());
        let child = cmd.spawn().expect("spawn");
        let mut guard = ChildGuard::new(child);
        let mut output = BufReader::new(guard.child().stdout.take().expect("stdout"));
        let mut line = String::new();
        output.read_line(&mut line).expect("read readiness");
        assert_eq!(line.trim(), "ready");
        let mut tree = ProcessTree::with_waiter_error(guard.child(), rustix::io::Errno::IO);
        assert!(tree.wait_exit(guard.child(), Duration::ZERO).is_err());
        tree.terminate().expect("terminate");
        let mut problems = Vec::new();
        assert!(
            tree.reap(guard.child(), &mut problems).is_some(),
            "{problems:?}"
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert!(guard
            .child()
            .try_wait()
            .expect("cached exit status")
            .is_some());
    });
}

#[cfg(windows)]
#[test]
fn setup_failure_reports_failed_kills_and_reap_timeout() {
    bounded(|| {
        let mut cmd = command(timeout_command());
        cmd.stdout(std::process::Stdio::piped());
        let (child, tree) = ProcessTree::spawn(&mut cmd).expect("spawn");
        let mut guard = ChildGuard::with_tree(child, tree);
        let mut output = BufReader::new(guard.child().stdout.take().expect("stdout"));
        let mut line = String::new();
        output.read_line(&mut line).expect("read readiness");
        assert_eq!(line.trim(), "ready");
        let error = crate::process_tree_windows::finish_setup(
            guard.child(),
            |_| Err(io::Error::other("injected setup failure")),
            |_| {
                (
                    Err(io::Error::other("injected tree kill failure")),
                    Err(io::Error::other("injected child kill failure")),
                )
            },
        )
        .expect_err("setup must fail");
        guard
            .tree
            .as_ref()
            .expect("guard owns tree")
            .terminate()
            .expect("cleanup kill");
        assert!(crate::supervise::bounded_reap(guard.child())
            .expect("cleanup reap")
            .is_some());
        let message = error.to_string();
        assert!(message.contains("injected setup failure"), "{message}");
        assert!(
            message.contains("tree kill: injected tree kill failure"),
            "{message}"
        );
        assert!(
            message.contains("child kill: injected child kill failure"),
            "{message}"
        );
        assert!(message.contains("leader could not be reaped"), "{message}");
    });
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
        let (child, tree) = ProcessTree::spawn(&mut cmd).expect("spawn");
        let mut guard = ChildGuard::with_tree(child, tree);
        let mut stdout = BufReader::new(guard.child().stdout.take().expect("stdout"));
        let mut line = String::new();
        if stdout.read_line(&mut line).expect("read readiness") == 0 {
            panic!("process exited before readiness handshake");
        }
        assert_eq!(line.trim(), "ready");
        drop(guard.tree.take());
        let mut remaining = Vec::new();
        stdout.read_to_end(&mut remaining).expect("read to EOF");
        assert!(crate::supervise::bounded_reap(guard.child())
            .expect("reap")
            .is_some());
    });
}
