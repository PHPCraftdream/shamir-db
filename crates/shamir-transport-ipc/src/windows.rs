//! Windows Named Pipe implementation of [`crate::IpcListener`] / [`crate::IpcStream`].
//!
//! Access boundary: every pipe instance is created with an explicit
//! security descriptor restricting access to the CURRENT process token's
//! user SID only (`D:P(A;;GA;;;<sid>)` — protected DACL, Generic-All to
//! that one SID). This is the Windows analogue of the Unix listener's
//! `chmod 0600`: parity is "the OS account this server runs as", not "the
//! current logon session" (a service account has no stable logon SID
//! across restarts, so the user SID — not `OWNER RIGHTS`/`OW` and not the
//! logon SID — is the correct, stable anchor for a long-lived daemon).
//! `ServerOptions::reject_remote_clients` (tokio's own default: `true`) is
//! left at its default, which additionally blocks the pipe from being
//! reachable over SMB from a remote host at all, regardless of DACL.
//!
//! A `NamedPipeServer` handle serves exactly ONE client, unlike a socket
//! listener — after a client connects, [`IpcListener::accept`] must create
//! a fresh pending instance before the next `accept` call, or a second
//! connecting client would find no listener present.

use std::ffi::c_void;
use std::io;
use std::ptr;

use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, PipeMode, ServerOptions,
};

use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_INSUFFICIENT_BUFFER, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// A connected Named Pipe instance — implements `AsyncRead + AsyncWrite`,
/// so `shamir_transport_tcp::framing`'s frame helpers work on it directly.
pub type IpcStream = NamedPipeServer;

/// Client-side connection type. A Named Pipe's client and server ends are
/// genuinely different OS handle types (unlike a Unix socket, where both
/// ends share one type) — `NamedPipeClient`, not `NamedPipeServer`.
pub type IpcClientStream = NamedPipeClient;

/// Connect to the Named Pipe server at `name` as a client. No explicit
/// security attributes needed here — `CreateFile` (which `ClientOptions::open`
/// wraps) checks the CALLING process's token against the pipe's own DACL
/// (set at server creation, see [`OwnerOnlySecurityDescriptor`]) automatically;
/// there is nothing analogous to configure on the client's open call.
pub async fn connect(name: &str) -> io::Result<IpcClientStream> {
    ClientOptions::new().open(name)
}

/// Owner-only-DACL Named Pipe listener. See module docs for the security
/// model.
pub struct IpcListener {
    name: String,
    sec_desc: OwnerOnlySecurityDescriptor,
    /// The next pending (unconnected) pipe instance. `Some` between
    /// construction / the end of one `accept` and the start of the next.
    next: Option<NamedPipeServer>,
}

impl IpcListener {
    /// Create a Named Pipe server at `name` (e.g. `\\.\pipe\shamir-db`),
    /// restricted to the current process's own user account.
    pub async fn bind(name: impl Into<String>) -> io::Result<Self> {
        let name = name.into();
        let sec_desc = OwnerOnlySecurityDescriptor::for_current_user()?;
        let first = create_instance(&name, &sec_desc, /* first_pipe_instance */ true)?;
        Ok(Self {
            name,
            sec_desc,
            next: Some(first),
        })
    }

    /// Accept the next incoming connection. Internally rotates in a fresh
    /// pending instance so a subsequent `accept` call can serve the NEXT
    /// client — a single `NamedPipeServer` handle is one-client-only,
    /// unlike a socket listener.
    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        let pending = self
            .next
            .take()
            .expect("IpcListener::accept: no pending instance — invariant violated");
        pending.connect().await?;
        self.next = Some(create_instance(
            &self.name,
            &self.sec_desc,
            /* first_pipe_instance */ false,
        )?);
        Ok(pending)
    }

    /// The pipe name this listener is bound to (e.g. `\\.\pipe\shamir-db`).
    pub fn path(&self) -> &str {
        &self.name
    }
}

fn create_instance(
    name: &str,
    sec_desc: &OwnerOnlySecurityDescriptor,
    first_pipe_instance: bool,
) -> io::Result<NamedPipeServer> {
    // SAFETY: `sec_desc.as_security_attributes()` returns a pointer to a
    // `SECURITY_ATTRIBUTES` whose `lpSecurityDescriptor` points at a valid,
    // still-owned `SECURITY_DESCRIPTOR` (owned by `sec_desc`, which
    // outlives this call — it is a field of `self`, not a temporary). Byte
    // framing / pipe mode are unaffected by the security descriptor.
    unsafe {
        ServerOptions::new()
            .pipe_mode(PipeMode::Byte)
            .first_pipe_instance(first_pipe_instance)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                name,
                sec_desc.as_security_attributes() as *mut c_void,
            )
    }
}

/// RAII owner of a Win32 `SECURITY_DESCRIPTOR` restricting access to the
/// current process token's user SID, plus the `SECURITY_ATTRIBUTES`
/// wrapper `CreateNamedPipe` expects. `ConvertStringSecurityDescriptorToSecurityDescriptorW`
/// allocates the descriptor via `LocalAlloc` — freed on `Drop` via
/// `LocalFree`.
struct OwnerOnlySecurityDescriptor {
    attrs: SECURITY_ATTRIBUTES,
}

// SAFETY: the owned `SECURITY_DESCRIPTOR` buffer is only ever read from
// (passed by pointer into `CreateNamedPipe`, in a call this crate makes
// under a `&self`/`&_` borrow) — no interior mutability, no thread-affine
// Win32 handle involved (unlike a `HANDLE`, a `LocalAlloc`-backed buffer
// has no thread-affinity requirement).
unsafe impl Send for OwnerOnlySecurityDescriptor {}
unsafe impl Sync for OwnerOnlySecurityDescriptor {}

impl OwnerOnlySecurityDescriptor {
    fn for_current_user() -> io::Result<Self> {
        let sid = current_user_sid_string()?;
        // `D:` = DACL, `P` = protected (blocks inheriting a laxer ACL from
        // a parent container), single ACE: Allow, Generic-All, to the
        // looked-up user SID.
        let sddl = format!("D:P(A;;GA;;;{sid})");
        let wide = to_wide_null(&sddl);

        let mut psd: *mut c_void = ptr::null_mut();
        // SAFETY: `wide` is a valid null-terminated UTF-16 string for the
        // duration of this call. `psd`/`size_out` are valid out-params.
        // On success `psd` is heap-allocated by the OS (`LocalAlloc`) and
        // owned by this struct from here on — freed in `Drop`.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut psd,
                ptr::null_mut(),
            )
        };
        if ok == 0 || psd.is_null() {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            attrs: SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: psd,
                bInheritHandle: 0,
            },
        })
    }

    fn as_security_attributes(&self) -> *const SECURITY_ATTRIBUTES {
        &self.attrs
    }
}

impl Drop for OwnerOnlySecurityDescriptor {
    fn drop(&mut self) {
        if !self.attrs.lpSecurityDescriptor.is_null() {
            // SAFETY: `lpSecurityDescriptor` was allocated by
            // `ConvertStringSecurityDescriptorToSecurityDescriptorW` (which
            // uses `LocalAlloc` internally per its own documented contract)
            // and is freed exactly once, here, when this — its sole owner
            // — is dropped.
            unsafe {
                LocalFree(self.attrs.lpSecurityDescriptor);
            }
        }
    }
}

/// String-SID (`S-1-5-21-...`) of the current process token's user —
/// i.e. the Windows account this server process is running as. Used as
/// the sole principal in every pipe instance's DACL.
fn current_user_sid_string() -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `GetCurrentProcess()` returns a pseudo-handle that needs no
    // closing. `OpenProcessToken` writes a real, owned handle into `token`
    // on success, closed below before returning.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        // First call to learn the required buffer size.
        let mut needed: u32 = 0;
        // SAFETY: null buffer + 0 length is the documented way to query
        // the required size; `GetTokenInformation` writes it to `needed`
        // and returns an expected `ERROR_INSUFFICIENT_BUFFER` failure.
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        let last_err = io::Error::last_os_error();
        if last_err.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(last_err);
        }

        let mut buf = vec![0u8; needed as usize];
        let mut actual: u32 = 0;
        // SAFETY: `buf` is `needed` bytes, matching what the prior call
        // reported as required for a `TOKEN_USER` at this token.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr() as *mut c_void,
                needed,
                &mut actual,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `buf` was just filled by `GetTokenInformation` with a
        // `TOKEN_USER` — `TOKEN_USER.User.Sid` is a valid `PSID` for the
        // lifetime of `buf` (it points INTO `buf`, not to a separate
        // allocation).
        let token_user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
        let sid = token_user.User.Sid;

        let mut sid_str_ptr: *mut u16 = ptr::null_mut();
        // SAFETY: `sid` is the valid PSID obtained above. On success the
        // OS allocates the string via `LocalAlloc`, freed just below.
        let ok = unsafe { ConvertSidToStringSidW(sid, &mut sid_str_ptr) };
        if ok == 0 || sid_str_ptr.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `sid_str_ptr` is a valid null-terminated UTF-16 string
        // freshly returned by `ConvertSidToStringSidW`.
        let len = unsafe {
            let mut n = 0isize;
            while *sid_str_ptr.offset(n) != 0 {
                n += 1;
            }
            n as usize
        };
        let slice = unsafe { std::slice::from_raw_parts(sid_str_ptr, len) };
        let sid_string = String::from_utf16_lossy(slice);
        // SAFETY: freeing the same `LocalAlloc`-backed buffer exactly once.
        unsafe {
            LocalFree(sid_str_ptr as *mut c_void);
        }
        Ok(sid_string)
    })();
    // SAFETY: `token` is a real, owned handle opened above — closed
    // exactly once, on every exit path (success or error) of this
    // function.
    unsafe {
        CloseHandle(token);
    }
    result
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
