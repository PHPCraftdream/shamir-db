use crate::{supervise, ProcessTree, TeardownFaults};
#[cfg(unix)]
use rustix::process::{kill_process_group, waitid, Pid, Signal, WaitId, WaitIdOptions};
use std::io::{self, BufRead, BufReader, Read};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
#[cfg(windows)]
use std::sync::Weak;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Watchdog {
    #[cfg(unix)]
    pgid: Option<Pid>,
    #[cfg(windows)]
    tree: Weak<ProcessTree>,
}

impl Watchdog {
    fn signal(&self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            // Every test-side reap and disarm holds this same mutex. NOWAIT
            // does not reap; the leader therefore pins the group through kill.
            let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
            if waitid(WaitId::Pid(pgid), options).is_ok() {
                let _ = kill_process_group(pgid, Signal::KILL);
            }
        }
        #[cfg(windows)]
        if let Some(tree) = self.tree.upgrade() {
            let _ = tree.terminate();
        }
    }

    fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.pgid = None;
        }
        #[cfg(windows)]
        {
            self.tree = Weak::new();
        }
    }
}

type SharedWatchdog = Arc<Mutex<Watchdog>>;

// Own the child until cleanup or transfer. On Unix the unreaped leader pins the
// process group; never signal it once another operation has reaped the leader.
struct ChildGuard {
    child: Option<Child>,
    #[cfg(unix)]
    pgid: Pid,
    #[cfg(unix)]
    tree: Option<ProcessTree>,
    #[cfg(windows)]
    tree: Option<Arc<ProcessTree>>,
    watchdog: SharedWatchdog,
}

impl ChildGuard {
    #[cfg(unix)]
    fn new(child: Child, watchdog: SharedWatchdog) -> Self {
        let pgid = Pid::from_child(&child);
        watchdog.lock().unwrap().pgid = Some(pgid);
        Self {
            child: Some(child),
            pgid,
            tree: None,
            watchdog,
        }
    }

    fn with_tree(child: Child, tree: ProcessTree, watchdog: SharedWatchdog) -> Self {
        #[cfg(unix)]
        let pgid = Pid::from_child(&child);
        #[cfg(windows)]
        let tree = Arc::new(tree);
        {
            let mut state = watchdog.lock().unwrap();
            #[cfg(unix)]
            {
                state.pgid = Some(pgid);
            }
            #[cfg(windows)]
            {
                state.tree = Arc::downgrade(&tree);
            }
        }
        Self {
            #[cfg(unix)]
            pgid,
            child: Some(child),
            tree: Some(tree),
            watchdog,
        }
    }

    fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("guard owns child")
    }

    fn transfer(&mut self) -> (Child, ProcessTree) {
        let mut state = self.watchdog.lock().unwrap();
        state.disarm();
        let child = self.child.take().expect("guard owns child");
        let tree = self.tree.take().expect("guard owns tree");
        #[cfg(windows)]
        let tree = Arc::try_unwrap(tree).unwrap_or_else(|_| panic!("tree still shared"));
        (child, tree)
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let mut state = self.watchdog.lock().unwrap_or_else(|e| e.into_inner());
        state.disarm();
        let Some(child) = self.child.as_mut() else {
            return;
        };
        #[cfg(unix)]
        {
            let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
            if waitid(WaitId::Pid(self.pgid), options).is_ok() {
                let _ = kill_process_group(self.pgid, Signal::KILL);
                let _ = child.kill();
                drop(self.tree.take());
            } else if let Some(tree) = self.tree.take() {
                // Its Drop would otherwise signal a potentially recycled PGID.
                std::mem::forget(tree);
            }
        }
        #[cfg(windows)]
        {
            if let Some(tree) = self.tree.as_ref() {
                let _ = tree.terminate();
            }
            let _ = child.kill();
        }
        drop(state);
        let _ = crate::supervise::bounded_reap(child);
    }
}

fn bounded<F: FnOnce(SharedWatchdog) + Send + 'static>(f: F) {
    bounded_for(Duration::from_secs(60), f);
}

fn bounded_for<F: FnOnce(SharedWatchdog) + Send + 'static>(timeout: Duration, f: F) {
    let watchdog = Arc::new(Mutex::new(Watchdog::default()));
    let worker_watchdog = Arc::clone(&watchdog);
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(worker_watchdog)));
        let _ = tx.send(result);
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => {
            let _ = worker.join();
            if let Err(payload) = result {
                std::panic::resume_unwind(payload);
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            if let Ok(state) = watchdog.try_lock() {
                state.signal();
            }
            let finished = rx.recv_timeout(Duration::from_secs(10)).is_ok();
            if finished {
                let _ = worker.join();
            }
            panic!("teardown hung (worker finished after cleanup: {finished})");
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("teardown worker disconnected"),
    }
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
    watchdog: SharedWatchdog,
) -> Result<std::process::Output, String> {
    let mut cmd = command(script);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (child, tree) = ProcessTree::spawn(&mut cmd).expect("spawn");
    let mut guard = ChildGuard::with_tree(child, tree, watchdog);
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
    let (child, tree) = guard.transfer();
    supervise(child, tree, Some(stdout), stderr, timeout, faults)
}

#[cfg(unix)]
#[test]
fn waiter_spawn_failure_reports_failed_kills_and_reap_timeout() {
    bounded(|watchdog| {
        let mut cmd = command(timeout_command());
        cmd.process_group(0).stdout(std::process::Stdio::piped());
        let child = cmd.spawn().expect("spawn");
        let mut guard = ChildGuard::new(child, watchdog);
        let mut output = BufReader::new(guard.child().stdout.take().expect("stdout"));
        let mut line = String::new();
        output.read_line(&mut line).expect("read readiness");
        assert_eq!(line.trim(), "ready");
        let pgid = Pid::from_child(guard.child());
        // start_waiter can reap internally. Disarm before calling it: holding
        // the mutex through its bounded wait would obstruct the controller.
        guard.watchdog.lock().unwrap().disarm();
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
    bounded(|watchdog| {
        let mut cmd = command(timeout_command());
        cmd.process_group(0).stdout(std::process::Stdio::piped());
        let child = cmd.spawn().expect("spawn");
        let mut guard = ChildGuard::new(child, watchdog);
        let mut output = BufReader::new(guard.child().stdout.take().expect("stdout"));
        let mut line = String::new();
        output.read_line(&mut line).expect("read readiness");
        assert_eq!(line.trim(), "ready");
        guard.watchdog.lock().unwrap().disarm();
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
    bounded(|watchdog| {
        let mut cmd = command(timeout_command());
        cmd.stdout(std::process::Stdio::piped());
        let (child, tree) = ProcessTree::spawn(&mut cmd).expect("spawn");
        let mut guard = ChildGuard::with_tree(child, tree, watchdog);
        let mut output = BufReader::new(guard.child().stdout.take().expect("stdout"));
        let mut line = String::new();
        output.read_line(&mut line).expect("read readiness");
        assert_eq!(line.trim(), "ready");
        guard.watchdog.lock().unwrap().disarm();
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
fn watchdog_unblocks_readiness_read() {
    let watchdog = Arc::new(Mutex::new(Watchdog::default()));
    let worker_watchdog = Arc::clone(&watchdog);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        #[cfg(unix)]
        let script = "sleep 1000";
        #[cfg(windows)]
        let script = "ping -n 1000 127.0.0.1 >NUL";
        let mut cmd = command(script);
        cmd.stdout(std::process::Stdio::piped());
        let (child, tree) = ProcessTree::spawn(&mut cmd).expect("spawn");
        let mut guard = ChildGuard::with_tree(child, tree, worker_watchdog);
        let mut output = BufReader::new(guard.child().stdout.take().expect("stdout"));
        let mut line = String::new();
        ready_tx.send(()).expect("controller present");
        let result = output.read_line(&mut line);
        let _ = done_tx.send(result.is_ok_and(|n| n == 0));
    });
    ready_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("worker registered");
    watchdog.lock().unwrap().signal();
    assert_eq!(
        done_rx.recv_timeout(Duration::from_secs(10)).ok(),
        Some(true)
    );
    worker.join().expect("worker completed");
}

#[test]
fn rv_wasm_host_58c052b0_timeout_kills_descendants_holding_pipes() {
    bounded(|watchdog| {
        let error = ready(
            timeout_command(),
            Duration::ZERO,
            TeardownFaults::default(),
            watchdog,
        )
        .unwrap_err();
        assert!(error.contains("compilation timed out"), "{error}");
        assert!(!error.contains("teardown incomplete"), "{error}");
    });
}
#[test]
fn rv_wasm_host_58c052b0_wait_error_kills_tree() {
    bounded(|watchdog| {
        let error = ready(
            timeout_command(),
            Duration::from_secs(600),
            TeardownFaults {
                fail_wait: true,
                ..Default::default()
            },
            watchdog,
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
    bounded(|watchdog| {
        let error = ready(
            timeout_command(),
            Duration::ZERO,
            TeardownFaults {
                fail_tree_kill: true,
                ..Default::default()
            },
            watchdog,
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
    bounded(|watchdog| {
        let output = ready(
            straggler_command(),
            Duration::from_secs(10),
            TeardownFaults::default(),
            watchdog,
        )
        .expect("supervision");
        assert!(output.status.success());
    });
}
#[test]
fn rv_wasm_host_58c052b0_dropping_tree_kills_descendants() {
    bounded(|watchdog| {
        let mut cmd = command(timeout_command());
        cmd.stdout(std::process::Stdio::piped());
        let (child, tree) = ProcessTree::spawn(&mut cmd).expect("spawn");
        let mut guard = ChildGuard::with_tree(child, tree, watchdog);
        let mut stdout = BufReader::new(guard.child().stdout.take().expect("stdout"));
        let mut line = String::new();
        if stdout.read_line(&mut line).expect("read readiness") == 0 {
            panic!("process exited before readiness handshake");
        }
        assert_eq!(line.trim(), "ready");
        drop(guard.tree.take());
        let mut remaining = Vec::new();
        stdout.read_to_end(&mut remaining).expect("read to EOF");
        guard.watchdog.lock().unwrap().disarm();
        assert!(crate::supervise::bounded_reap(guard.child())
            .expect("reap")
            .is_some());
    });
}
