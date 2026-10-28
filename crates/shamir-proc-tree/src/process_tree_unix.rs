use rustix::process::{kill_process_group, waitid, Pid, Signal, WaitId, WaitIdOptions};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

pub struct ProcessTree {
    pgid: Pid,
    exit: Receiver<rustix::io::Result<()>>,
    exit_seen: bool,
    reaped: bool,
}

impl ProcessTree {
    pub fn spawn(cmd: &mut Command) -> io::Result<(Child, Self)> {
        let child = cmd.process_group(0).spawn()?;
        let pgid = Pid::from_child(&child);
        let (tx, rx) = mpsc::sync_channel(1);
        if let Err(error) = thread::Builder::new()
            .name("process-tree-waiter".into())
            .spawn(move || {
                let result: rustix::io::Result<()> = loop {
                    match waitid(
                        WaitId::Pid(pgid),
                        WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
                    ) {
                        Ok(_) => break Ok(()),
                        Err(rustix::io::Errno::INTR) => continue,
                        Err(error) => break Err(error),
                    }
                };
                let _ = tx.send(result);
            })
        {
            let _ = kill_process_group(pgid, Signal::KILL);
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok((
            child,
            Self {
                pgid,
                exit: rx,
                exit_seen: false,
                reaped: false,
            },
        ))
    }

    pub(crate) fn wait_exit(&mut self, _child: &mut Child, timeout: Duration) -> io::Result<bool> {
        if self.exit_seen {
            return Ok(true);
        }
        match self.exit.recv_timeout(timeout) {
            Ok(Ok(_)) => {
                self.exit_seen = true;
                Ok(true)
            }
            Ok(Err(error)) => Err(io::Error::from(error)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(false),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(io::Error::other("process waiter disconnected"))
            }
        }
    }

    pub(crate) fn terminate(&self) -> io::Result<()> {
        if self.reaped {
            return Ok(());
        }
        match kill_process_group(self.pgid, Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(error) => Err(io::Error::from(error)),
        }
    }

    pub(crate) fn reap(
        &mut self,
        child: &mut Child,
        problems: &mut Vec<String>,
    ) -> Option<ExitStatus> {
        if !self.exit_seen {
            problems.push("leader not reaped".into());
            return None;
        }
        if let Err(error) = self.terminate() {
            problems.push(format!("final tree kill: {error}"));
        }
        match child.wait() {
            Ok(status) => {
                self.reaped = true;
                Some(status)
            }
            Err(error) => {
                problems.push(format!("leader reap: {error}"));
                None
            }
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}
