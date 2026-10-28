#![allow(unsafe_code)]
#![warn(clippy::undocumented_unsafe_blocks)]

use std::io;
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::process::Child;
use std::ptr;
use windows_sys::Win32::Foundation::{GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

pub(crate) struct JobObject(OwnedHandle);
impl JobObject {
    pub(crate) fn kill_on_close() -> io::Result<Self> {
        // SAFETY: null security attributes/name request an unnamed job; result is an owned kernel handle.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(last_error());
        }
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: initialized info and exact size are passed with a live handle.
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *mut core::ffi::c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            let error = last_error();
            // SAFETY: the CreateJobObjectW handle is still owned here and is closed on this failure path.
            drop(unsafe { OwnedHandle::from_raw_handle(handle) });
            return Err(error);
        }
        // SAFETY: successful creation returned a valid HANDLE transferred into this owner.
        let owned = unsafe { OwnedHandle::from_raw_handle(handle) };
        Ok(Self(owned))
    }
    pub(crate) fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: handles are live; child remains suspended until job assignment.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }
    pub(crate) fn terminate(&self) -> io::Result<()> {
        // SAFETY: the owned job handle remains live during the call.
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }
    pub(crate) fn resume_primary_thread(&self, pid: u32) -> io::Result<()> {
        // SAFETY: valid snapshot flags and no pointer arguments.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(last_error());
        }
        // SAFETY: successful snapshot is an owned kernel handle.
        let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        // SAFETY: entry is initialized/writable and snapshot is valid.
        let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
        while found {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: thread id comes from the live system snapshot.
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if thread.is_null() {
                    return Err(last_error());
                }
                // SAFETY: OpenThread returned an owned thread handle.
                let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
                // SAFETY: thread handle has THREAD_SUSPEND_RESUME access.
                if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                    return Err(last_error());
                }
                return Ok(());
            }
            // SAFETY: entry remains writable and snapshot remains live.
            found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "primary thread not found",
        ))
    }
}
fn last_error() -> io::Error {
    // SAFETY: GetLastError takes no arguments and returns calling-thread status.
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}
