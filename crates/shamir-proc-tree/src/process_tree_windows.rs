use crate::job_object_windows::JobObject;
use std::io;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::time::Duration;
use wait_timeout::ChildExt;

/// Closing the owned job handle terminates its processes (`KILL_ON_JOB_CLOSE`).
pub struct ProcessTree {
    job: JobObject,
    status: Option<ExitStatus>,
}

pub(super) fn finish_setup(
    child: &mut Child,
    setup: impl FnOnce(&Child) -> io::Result<()>,
    kill: impl FnOnce(&mut Child) -> (io::Result<()>, io::Result<()>),
) -> io::Result<()> {
    if let Err(error) = setup(child) {
        let (tree_kill, child_kill) = kill(child);
        return Err(crate::supervise::startup_cleanup(
            child, error, tree_kill, child_kill,
        ));
    }
    Ok(())
}

impl ProcessTree {
    pub fn spawn(cmd: &mut Command) -> io::Result<(Child, Self)> {
        let job = JobObject::kill_on_close()?;
        let mut child = cmd
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED)
            .spawn()?;
        finish_setup(
            &mut child,
            |child| {
                job.assign(child)
                    .and_then(|()| job.resume_primary_thread(child.id()))
            },
            |child| {
                let tree_kill = job.terminate();
                let child_kill = child.kill();
                (tree_kill, child_kill)
            },
        )?;
        Ok((child, Self { job, status: None }))
    }
    pub(crate) fn wait_exit(&mut self, child: &mut Child, timeout: Duration) -> io::Result<bool> {
        match child.wait_timeout(timeout) {
            Ok(Some(status)) => {
                self.status = Some(status);
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(error) => {
                if let Some(status) = child.try_wait()? {
                    self.status = Some(status);
                }
                Err(error)
            }
        }
    }
    pub(crate) fn terminate(&self) -> io::Result<()> {
        self.job.terminate()
    }
    pub(crate) fn reap(
        &mut self,
        child: &mut Child,
        problems: &mut Vec<String>,
    ) -> Option<ExitStatus> {
        if self.status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => self.status = Some(status),
                Ok(None) => {
                    problems.push("leader not reaped".into());
                    return None;
                }
                Err(error) => {
                    problems.push(format!("leader reap: {error}"));
                    return None;
                }
            }
        }
        self.status
    }
}
