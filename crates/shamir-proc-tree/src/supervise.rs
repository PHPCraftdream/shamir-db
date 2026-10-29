use std::io::{self, Read};
use std::process::{Child, Output};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const GRACE: Duration = Duration::from_secs(5);

pub(super) fn poll_until<T>(
    mut poll: impl FnMut() -> io::Result<Option<T>>,
) -> io::Result<Option<T>> {
    let deadline = Instant::now() + GRACE;
    loop {
        if let Some(value) = poll()? {
            return Ok(Some(value));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        thread::sleep(remaining.min(Duration::from_millis(5)));
    }
}

pub(super) fn bounded_reap(child: &mut Child) -> io::Result<Option<std::process::ExitStatus>> {
    poll_until(|| child.try_wait())
}

pub(super) fn startup_cleanup(
    child: &mut Child,
    error: io::Error,
    tree_kill: io::Result<()>,
    child_kill: io::Result<()>,
) -> io::Error {
    let mut problems = Vec::new();
    if let Err(error) = tree_kill {
        problems.push(format!("tree kill: {error}"));
    }
    if let Err(error) = child_kill {
        problems.push(format!("child kill: {error}"));
    }
    match bounded_reap(child) {
        Ok(Some(_)) => {}
        Ok(None) => problems.push("leader could not be reaped within 5s".into()),
        Err(error) => problems.push(format!("leader reap: {error}")),
    }
    if problems.is_empty() {
        error
    } else {
        io::Error::new(
            error.kind(),
            format!("{error}; teardown incomplete: {}", problems.join("; ")),
        )
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct TeardownFaults {
    pub fail_tree_kill: bool,
    pub fail_wait: bool,
}

struct PipeReader {
    name: &'static str,
    rx: Option<Receiver<Vec<u8>>>,
    bytes: Option<Vec<u8>>,
}
impl PipeReader {
    fn start<R: Read + Send + 'static>(name: &'static str, reader: Option<R>) -> io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name(format!("{name}-reader"))
            .spawn(move || {
                let mut bytes = Vec::new();
                if let Some(mut reader) = reader {
                    let _ = reader.read_to_end(&mut bytes);
                }
                let _ = tx.send(bytes);
            })?;
        Ok(Self {
            name,
            rx: Some(rx),
            bytes: None,
        })
    }
    /// Placeholder for a reader whose thread never started: nothing to drain.
    fn absent(name: &'static str) -> Self {
        Self {
            name,
            rx: None,
            bytes: None,
        }
    }
    fn collect(&mut self, deadline: Instant) -> bool {
        let Some(rx) = self.rx.as_ref() else {
            return false;
        };
        if self.bytes.is_some() {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(bytes) => {
                self.bytes = Some(bytes);
                false
            }
            Err(_) => true,
        }
    }
    fn take(&mut self) -> Vec<u8> {
        self.bytes.take().unwrap_or_default()
    }
}

enum Outcome {
    Exited,
    TimedOut,
    Failed(String),
}

pub fn supervise<O: Read + Send + 'static, E: Read + Send + 'static>(
    mut child: Child,
    mut tree: crate::ProcessTree,
    stdout: Option<O>,
    stderr: Option<E>,
    timeout: Duration,
    faults: TeardownFaults,
) -> Result<Output, String> {
    let mut spawn_failure = None;
    let mut out = PipeReader::start("stdout", stdout).unwrap_or_else(|error| {
        spawn_failure.get_or_insert(format!("pipe reader spawn failed: {error}"));
        PipeReader::absent("stdout")
    });
    let mut err = PipeReader::start("stderr", stderr).unwrap_or_else(|error| {
        spawn_failure.get_or_insert(format!("pipe reader spawn failed: {error}"));
        PipeReader::absent("stderr")
    });
    let outcome = if let Some(message) = spawn_failure {
        Outcome::Failed(message)
    } else if faults.fail_wait {
        Outcome::Failed("cargo wait failed: injected wait failure".into())
    } else {
        match tree.wait_exit(&mut child, timeout) {
            Ok(true) => Outcome::Exited,
            Ok(false) => Outcome::TimedOut,
            Err(error) => Outcome::Failed(format!("cargo wait failed: {error}")),
        }
    };
    let mut problems = Vec::new();
    if !matches!(outcome, Outcome::Exited) {
        kill_tree(&tree, &mut child, faults.fail_tree_kill, &mut problems);
        match tree.wait_exit(&mut child, GRACE) {
            Ok(true) => {}
            Ok(false) => problems.push("leader did not exit within 5s".into()),
            Err(error) => problems.push(format!("leader wait: {error}")),
        }
    }
    let deadline = Instant::now() + GRACE;
    let mut out_pending = out.collect(deadline);
    let mut err_pending = err.collect(deadline);
    if matches!(outcome, Outcome::Exited) && (out_pending || err_pending) {
        kill_tree(&tree, &mut child, faults.fail_tree_kill, &mut problems);
        let deadline = Instant::now() + GRACE;
        out_pending = out.collect(deadline);
        err_pending = err.collect(deadline);
    }
    if out_pending {
        problems.push(format!("{} drain timed out", out.name));
    }
    if err_pending {
        problems.push(format!("{} drain timed out", err.name));
    }
    let status = tree.reap(&mut child, &mut problems);
    let output = status.map(|status| Output {
        status,
        stdout: out.take(),
        stderr: err.take(),
    });
    match outcome {
        Outcome::TimedOut => Err(
            format!("compilation timed out after {}s", timeout.as_secs()) + &teardown(&problems),
        ),
        Outcome::Failed(message) => Err(message + &teardown(&problems)),
        Outcome::Exited if !problems.is_empty() => Err(format!(
            "cargo build teardown incomplete: {}",
            problems.join("; ")
        )),
        Outcome::Exited => output.ok_or_else(|| {
            format!(
                "cargo build teardown incomplete: leader status unavailable{}",
                teardown(&problems)
            )
        }),
    }
}
fn kill_tree(
    tree: &crate::ProcessTree,
    child: &mut Child,
    injected: bool,
    problems: &mut Vec<String>,
) {
    let result = if injected {
        Err(io::Error::other("injected tree kill failure"))
    } else {
        tree.terminate()
    };
    if let Err(error) = result {
        log::error!("process tree kill failed: {error}");
        problems.push(format!("tree kill: {error}"));
        if let Err(error) = child.kill() {
            problems.push(format!("child kill: {error}"));
        }
    }
}
fn teardown(problems: &[String]) -> String {
    if problems.is_empty() {
        String::new()
    } else {
        format!("; teardown incomplete: {}", problems.join("; "))
    }
}
