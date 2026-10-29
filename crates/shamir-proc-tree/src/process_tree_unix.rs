use rustix::io::Errno;
use rustix::process::{
    kill_process_group, test_kill_process_group, waitid, Pid, Signal, WaitId, WaitIdOptions,
};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

/// Darwin `killpg` skips zombies and returns EPERM when no live member was signalled.
const ZOMBIE_EPERM: bool = cfg!(target_vendor = "apple");

pub struct ProcessTree {
    pgid: Pid,
    exit: Receiver<rustix::io::Result<()>>,
    exit_seen: bool,
    reaped: bool,
}

impl ProcessTree {
    #[cfg(test)]
    pub(super) fn with_waiter_error(child: &Child, error: Errno) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(Err(error)).expect("inject waiter error");
        Self {
            pgid: Pid::from_child(child),
            exit: rx,
            exit_seen: false,
            reaped: false,
        }
    }

    pub fn spawn(cmd: &mut Command) -> io::Result<(Child, Self)> {
        let mut child = cmd.process_group(0).spawn()?;
        let pgid = Pid::from_child(&child);
        let (tx, rx) = mpsc::sync_channel(1);
        start_waiter(
            &mut child,
            || {
                thread::Builder::new()
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
                    .map(|_| ())
            },
            |child| {
                let tree_kill = kill_process_group(pgid, Signal::KILL).map_err(io::Error::from);
                let child_kill = child.kill();
                (tree_kill, child_kill)
            },
        )?;
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
        // The unreaped leader pins the pgid, so this kill cannot hit a reused group.
        let unverified = match kill_process_group(self.pgid, Signal::KILL) {
            Ok(()) | Err(Errno::SRCH) => false,
            Err(Errno::PERM) if ZOMBIE_EPERM => true,
            Err(error) => {
                problems.push(format!("final tree kill: {}", io::Error::from(error)));
                false
            }
        };
        match crate::supervise::bounded_reap(child) {
            Ok(Some(status)) => {
                self.reaped = true;
                if unverified && !self.group_gone() {
                    problems.push("final tree kill: live member not signallable (EPERM)".into());
                }
                Some(status)
            }
            Ok(None) => {
                problems.push("leader could not be reaped within 5s".into());
                if unverified {
                    problems.push("final tree kill: EPERM unverified".into());
                }
                None
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
        crate::supervise::poll_until(|| match test_kill_process_group(self.pgid) {
            Err(Errno::SRCH) => Ok(Some(())),
            Err(Errno::PERM) => Ok(None),
            _ => Err(io::Error::other("group still exists")),
        })
        .is_ok_and(|result| result.is_some())
    }
}

pub(super) fn start_waiter(
    child: &mut Child,
    spawn: impl FnOnce() -> io::Result<()>,
    kill: impl FnOnce(&mut Child) -> (io::Result<()>, io::Result<()>),
) -> io::Result<()> {
    if let Err(error) = spawn() {
        let (tree_kill, child_kill) = kill(child);
        return Err(crate::supervise::startup_cleanup(
            child, error, tree_kill, child_kill,
        ));
    }
    Ok(())
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}
