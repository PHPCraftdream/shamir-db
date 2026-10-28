use rustix::io::Errno;
use rustix::process::{
    kill_process_group, test_kill_process_group, waitid, Pid, Signal, WaitId, WaitIdOptions,
};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// Darwin `killpg` skips zombies and returns EPERM when no live member was signalled.
const ZOMBIE_EPERM: bool = cfg!(target_vendor = "apple");
/// Bound for confirming a zombie-only group vanished after the leader reap.
const GONE_PROBE: Duration = Duration::from_secs(5);

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
                        Err(Errno::INTR) => continue,
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
            Ok(()) | Err(Errno::SRCH) => Ok(()),
            // Exited leader: ambiguous on Darwin; `reap` verifies the group is gone.
            Err(Errno::PERM) if ZOMBIE_EPERM && self.leader_exited() => Ok(()),
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
        // The zombie leader pins the pgid, so this kill cannot hit a reused group.
        let unverified = match kill_process_group(self.pgid, Signal::KILL) {
            Ok(()) | Err(Errno::SRCH) => false,
            Err(Errno::PERM) if ZOMBIE_EPERM => true,
            Err(error) => {
                problems.push(format!("final tree kill: {}", io::Error::from(error)));
                false
            }
        };
        match child.wait() {
            Ok(status) => {
                self.reaped = true;
                if unverified && !self.group_gone() {
                    problems.push("final tree kill: live member not signallable (EPERM)".into());
                }
                Some(status)
            }
            Err(error) => {
                problems.push(format!("leader reap: {error}"));
                if unverified {
                    problems.push("final tree kill: EPERM unverified".into());
                }
                None
            }
        }
    }

    fn leader_exited(&self) -> bool {
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        matches!(waitid(WaitId::Pid(self.pgid), options), Ok(Some(_)))
    }

    /// After the leader reap: a zombie-only group vanishes (ESRCH); lasting EPERM means a live member.
    fn group_gone(&self) -> bool {
        let deadline = Instant::now() + GONE_PROBE;
        loop {
            match test_kill_process_group(self.pgid) {
                Err(Errno::SRCH) => return true,
                Err(Errno::PERM) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                _ => return false,
            }
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}
