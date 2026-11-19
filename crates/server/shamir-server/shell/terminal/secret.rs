//! Secret source selection and the shared bounded line reader.
//!
//! Design: `docs/dev-artifacts/design/admin-shell.md` §3.2 (mutually
//! exclusive sources, 4096-byte cap, TTY refused as `--secret-stdin` /
//! `--secret-fd`, fixed `SHAMIR_SHELL_PASSWORD` variable), §6 (zeroize on
//! every path, no secret in errors or Debug, no history). The platform
//! modules (`unix.rs`, `windows.rs`) implement the no-echo hidden-line
//! session behind the [`Terminal`] seam; this file owns source dispatch,
//! the prompt orchestration (disable → read → restore) and the bounded
//! reader, so every source — prompt, stdin, fd, env — hits the same
//! 4096-byte + CRLF limit path and never keeps an unzeroized copy.

use std::io::{IsTerminal, Write};
#[cfg(unix)]
use std::os::fd::{FromRawFd, OwnedFd};

#[cfg(windows)]
use std::os::windows::io::AsRawHandle;

use zeroize::Zeroizing;

use crate::shell::output::ShellError;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF, HANDLE, INVALID_HANDLE_VALUE,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::ReadFile;

#[cfg(unix)]
use super::unix;
#[cfg(windows)]
use super::windows;

/// The one environment variable the shell reads secrets from (design §3.2:
/// allow-listed to exactly the auth-matched fixed variable; the parser
/// validates the name upstream and only this constant reaches this module).
const SECRET_ENV_VAR: &str = "SHAMIR_SHELL_PASSWORD";

/// Hard cap from design §3.2 (`BSTDIN` precedent): a longer input is a typed
/// error, never a truncated secret.
pub(super) const SECRET_MAX_BYTES: usize = 4096;

/// One secret source (design §3.2). At most one may be selected; the parser
/// enforces exclusivity, this module enforces per-source rules.
///
/// `Env` is deliberately a unit variant: the name is fixed to
/// [`SECRET_ENV_VAR`], never operator-supplied.
///
/// `Fd` carries the raw descriptor; on Windows the variant still exists so
/// the flag parses, but [`SecretSource::Fd`] is refused before any platform
/// code sees it (design: `--secret-fd` is Unix-only).
pub(crate) enum SecretSource {
    /// Hidden TTY prompt (interactive mode, or explicit `--secret-prompt`).
    Prompt,
    /// Inherited fd 0 on both OSes; refused when fd 0 is a TTY.
    Stdin,
    /// Unix-only: read from an already-open descriptor via a cloexec
    /// duplicate; only the duplicate is ever closed.
    Fd(i32),
    /// Fixed variable [`SECRET_ENV_VAR`].
    Env,
}

/// Platform no-echo session seam (test seam, design §8: the terminal layer
/// sits behind a trait so a scripted fake drives the real orchestration).
///
/// [`Terminal::begin_hidden_read`] is the *disable* step; the returned
/// session exposes the *read* step and the *restore* step. The orchestration
/// in [`prompt_secret`] always calls restore — including on the read-error
/// path — and reports a restore failure ahead of the read outcome, so a fake
/// can verify the disable → read → restore ordering on success and failure
/// alike.
pub(crate) trait Terminal {
    /// Disable echo (the terminal must be restored afterwards, by
    /// [`HiddenLineSession::restore`] or the implementation's `Drop`
    /// backstop), and return the hidden-line session.
    fn begin_hidden_read(&self) -> Result<Box<dyn HiddenLineSession + '_>, ShellError>;
}

/// One acquired no-echo session: read the hidden line, then restore.
pub(crate) trait HiddenLineSession {
    /// Read one hidden line while echo is disabled. Borrowed (`&mut self`)
    /// so the caller can still consume the session with
    /// [`HiddenLineSession::restore`] afterwards. Implementations must
    /// recognize the interrupt key (Unix byte 3 with `ISIG` off, Windows
    /// `0x03` character with `ENABLE_PROCESSED_INPUT` off) as
    /// [`ShellError::Interrupted`], and keep every scratch buffer zeroized.
    fn read_line(&mut self) -> Result<Zeroizing<Vec<u8>>, ShellError>;
    /// Restore the terminal mode captured at disable time. The
    /// implementation's `Drop` backstop retries a failed restore, so the
    /// explicit call only decides which error is reported first.
    fn restore(self: Box<Self>) -> Result<(), ShellError>;
}

/// Scripted-regression seam (agreed interface, used by
/// `parser::run_scripted`): identical dispatch, but the hidden prompt's
/// bytes ("password: " and the closing newline) go to the injected writer
/// instead of the process stderr, so the sibling test can observe the
/// disable → prompt-write → read → restore ordering across BOTH seams on
/// one captured timeline. Non-prompt sources ignore the writer entirely.
pub(crate) fn read_secret_with_writer(
    source: SecretSource,
    terminal: &dyn Terminal,
    stderr: &mut dyn Write,
) -> Result<Zeroizing<Vec<u8>>, ShellError> {
    match source {
        SecretSource::Prompt => prompt_secret(terminal, stderr),
        SecretSource::Stdin => read_stdin_secret(),
        SecretSource::Fd(fd) => read_fd_secret(fd),
        SecretSource::Env => read_env_secret(),
    }
}

/// The real platform terminal (Unix `rustix::termios`, Windows console API).
/// `pub(crate)` so the parser's scripted context can hold it as the
/// production [`Terminal`] choice.
pub(crate) fn os_terminal() -> &'static dyn Terminal {
    #[cfg(unix)]
    {
        &unix::OsTerminal
    }
    #[cfg(windows)]
    {
        &windows::OsTerminal
    }
    #[cfg(not(any(unix, windows)))]
    {
        compile_error!("the admin shell supports only unix and windows targets")
    }
}

/// The prompt orchestration: disable FIRST (echo must already be off when
/// the prompt text hits the terminal — design §3.2 "hidden TTY prompt with
/// echo disabled"; a prompt echoed back would leak that a prompt is active
/// and, on a terminal where the operator types ahead, input visible), then
/// write+flush the prompt to stderr (human text is stderr-only, design §5),
/// then read → restore in that order. Restore happens on every path after a
/// successful disable: prompt-write failure, read success, read error alike
/// (a panic unwind is covered by the implementation's `Drop` backstop, see
/// the [`Terminal`] seam docs). A restore failure is reported ahead of the
/// prompt or read outcome because the terminal being left with echo
/// disabled is the more urgent fault.
fn prompt_secret(
    terminal: &dyn Terminal,
    stderr: &mut dyn Write,
) -> Result<Zeroizing<Vec<u8>>, ShellError> {
    // Echo is off before any prompt byte is written or flushed.
    let mut session = terminal.begin_hidden_read()?;
    let prompt_result = stderr
        .write_all(b"password: ")
        .and_then(|()| stderr.flush());
    if prompt_result.is_err() {
        // Prompt failure after disable: restore, then report — restore
        // failure outranks the prompt failure (same precedence rule as
        // below).
        return match session.restore() {
            Ok(()) => Err(ShellError::SecretTerminal),
            Err(restore) => Err(restore),
        };
    }
    let read_result = session.read_line();
    let restore_result = session.restore();
    let _ = stderr.write_all(b"\n");
    let _ = stderr.flush();
    match (read_result, restore_result) {
        (Ok(secret), Ok(())) => Ok(secret),
        (_, Err(restore)) => Err(restore),
        (Err(read), Ok(())) => Err(read),
    }
}

/// `--secret-stdin`: inherited fd 0, both OSes. A TTY on fd 0 is refused
/// (design §3.2) — this also keeps the interactive line reader on stdin
/// alive for the command loop. The read is unbuffered byte-at-a-time through
/// the common bounded reader (no `BufReader` that could swallow the next
/// line of the interactive loop or buffer secret bytes outside zeroized
/// storage).
/// `read_stdin_byte` (per-OS, below) supplies the bytes.
fn read_stdin_secret() -> Result<Zeroizing<Vec<u8>>, ShellError> {
    if std::io::stdin().is_terminal() {
        return Err(ShellError::InsecureSecret);
    }
    read_line_bounded(read_stdin_byte)
}

/// One unbuffered byte from fd 0 through the bounded reader (no `BufReader`
/// that could swallow the next line of the interactive loop or buffer secret
/// bytes outside zeroized storage). Returns `None` on a clean EOF; any read
/// failure is a typed refusal, never a partial secret.
#[cfg(unix)]
fn read_stdin_byte() -> Result<Option<u8>, ShellError> {
    // `Stdin` implements `AsFd`; `rustix::io::read` is a raw `read(2)` on the
    // borrowed descriptor — no buffering layer involved. The one-byte scratch
    // is zeroized.
    let stdin = std::io::stdin();
    let mut byte = Zeroizing::new([0u8; 1]);
    let read = rustix::io::read(&stdin, &mut byte[..]).map_err(|_| ShellError::InvalidSecret)?;
    if read == 0 {
        Ok(None)
    } else {
        Ok(Some(byte[0]))
    }
}

/// Windows counterpart: one byte straight from the standard-input handle via
/// `ReadFile`. std's `StdinLock` is internally buffered (a `BufReader` that
/// would keep an unzeroized aggregate copy of the secret), so the raw
/// validated handle is used instead. Broken pipe / read failure and EOF are
/// handled distinctly: failure is a typed refusal, EOF ends the line.
#[cfg(windows)]
fn read_stdin_byte() -> Result<Option<u8>, ShellError> {
    let stdin = std::io::stdin();
    let handle = stdin.as_raw_handle() as HANDLE;
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(ShellError::InvalidSecret);
    }
    let mut byte = Zeroizing::new([0u8; 1]);
    let mut got: u32 = 0;
    // SAFETY: `handle` is the live, validated standard-input handle; the
    // one-byte zeroized scratch is the `*mut u8` in-buffer with `got` as the
    // read-count out-pointer and a null `*mut OVERLAPPED` (synchronous
    // handle, no overlapped I/O). `ERROR_BROKEN_PIPE` and `ERROR_HANDLE_EOF`
    // are clean end-of-input, mapped to `Ok(None)`; every other failure is a
    // typed refusal, never a partial secret.
    if unsafe {
        ReadFile(
            handle,
            byte.as_mut_ptr(),
            1,
            &mut got,
            core::ptr::null_mut(),
        )
    } == 0
    {
        // SAFETY: `GetLastError` takes no arguments and owns no pointers.
        let err = unsafe { GetLastError() };
        if err == ERROR_BROKEN_PIPE || err == ERROR_HANDLE_EOF {
            return Ok(None);
        }
        return Err(ShellError::InvalidSecret);
    }
    if got == 0 {
        Ok(None)
    } else {
        Ok(Some(byte[0]))
    }
}

/// POSIX `dup` for the `--secret-fd` duplicate; declared locally because the
/// workspace deliberately carries no `libc` crate dependency.
#[cfg(unix)]
extern "C" {
    fn dup(fd: core::ffi::c_int) -> core::ffi::c_int;
}

/// `--secret-fd N` (Unix only): duplicate N with close-on-exec, read, then
/// drop only the duplicate — the caller's descriptor is never touched, and
/// no `BorrowedFd`/`OwnedFd` is ever fabricated from the arbitrary integer.
///
/// `OwnedFd::try_from(RawFd)` is not used: std offers no try-style duplicate,
/// and `From RawFd` of an unverified integer would close an arbitrary
/// descriptor on drop. Instead `dup(2)` runs first and only a value `>= 0`
/// becomes an `OwnedFd`. The close-on-exec flag is set after the duplicate
/// exists; the process never spawns children while a prompt or secret read
/// is in flight, so the flag-set window cannot leak into an exec.
#[cfg(unix)]
fn read_fd_secret(raw_fd: i32) -> Result<Zeroizing<Vec<u8>>, ShellError> {
    // SAFETY: `dup` is a POSIX function taking one integer and owning no
    // pointers; on success it returns a NEW descriptor the caller owns, on
    // failure -1 with errno.
    let duplicated = unsafe { dup(raw_fd) };
    if duplicated < 0 {
        return Err(ShellError::InvalidSecret);
    }
    // SAFETY: `duplicated >= 0` is, per POSIX `dup`, a descriptor this
    // process newly owns; it is wrapped once and closed exactly once (drop).
    let duplicate = unsafe { OwnedFd::from_raw_fd(duplicated) };
    // The flag-set window is bounded to this call (see the doc comment).
    if rustix::io::fcntl_setfd(&duplicate, rustix::io::FdFlags::CLOEXEC).is_err() {
        return Err(ShellError::InvalidSecret);
    }
    if rustix::termios::isatty(&duplicate) {
        return Err(ShellError::InsecureSecret);
    }
    read_line_bounded(|| {
        let mut byte = Zeroizing::new([0u8; 1]);
        let read =
            rustix::io::read(&duplicate, &mut byte[..]).map_err(|_| ShellError::InvalidSecret)?;
        if read == 0 {
            Ok(None)
        } else {
            Ok(Some(byte[0]))
        }
    })
}

/// `--secret-fd N` on Windows: typed refusal with a pointer to
/// `--secret-stdin` (design §3.2); the flag parses but is never honored.
#[cfg(windows)]
fn read_fd_secret(_raw_fd: i32) -> Result<Zeroizing<Vec<u8>>, ShellError> {
    Err(ShellError::SecretFdWindows)
}

/// `--secret-env`: the fixed variable only. The raw value is wrapped in a
/// zeroizing buffer the moment it leaves the environment; the variable is
/// deliberately NOT removed (no global mutation of the process environment,
/// design §6: the same-user visibility window while the variable is set is
/// documented). Embedded line breaks are refused, so no downstream
/// line-oriented consumer can silently truncate the secret to its first
/// line; the value then flows through the same bounded reader as every
/// other source.
fn read_env_secret() -> Result<Zeroizing<Vec<u8>>, ShellError> {
    let value = Zeroizing::new(
        std::env::var_os(SECRET_ENV_VAR)
            .ok_or(ShellError::InvalidSecret)?
            .into_encoded_bytes(),
    );
    if value.is_empty() {
        return Err(ShellError::InvalidSecret);
    }
    if value.contains(&b'\n') || value.contains(&b'\r') {
        return Err(ShellError::InvalidSecret);
    }
    let mut offset = 0usize;
    read_line_bounded(|| {
        // The iterator ends with `Ok(None)` — EOF, not a refusal — so a
        // complete valid value is never rejected at its end.
        let Some(&byte) = value.get(offset) else {
            return Ok(None);
        };
        offset += 1;
        Ok(Some(byte))
    })
}

/// The one bounded reader every source uses: 4096-byte cap (overflow is a
/// typed error, never a truncated secret), the buffer is allocated once at
/// `SECRET_MAX_BYTES + 1` capacity (a growing secret never reallocates and
/// leaves heap copies behind). The spare byte holds a CR at the cap ONLY
/// until the next byte decides its fate: an LF terminator pops exactly that
/// one CR and the line lands back at the cap; anything else (another byte,
/// or EOF with the CR unpaired) is 4097 content bytes and is refused. It
/// strips EXACTLY ONE LF or CRLF line terminator and nothing else: a bare
/// CR at EOF is content and is kept, and in CRCRLF only the second CR (the
/// one paired with the LF) is popped — the first CR stays. Interior CR/LF
/// bytes are never touched.
/// Validation borrows
/// the accumulated bytes (`str::from_utf8`) — no unzeroized clone or temp
/// `String` is created. The accumulator is zeroized on every path, including
/// the error paths.
///
/// `pub(crate)` is the universal test seam: `tests/admin/main.rs` includes
/// this production tree via `#[path]`, so the scripted tests drive the real
/// reader byte-for-byte on every OS without a fake fd or a duplicated copy.
pub(crate) fn read_line_bounded(
    mut next_byte: impl FnMut() -> Result<Option<u8>, ShellError>,
) -> Result<Zeroizing<Vec<u8>>, ShellError> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(SECRET_MAX_BYTES + 1));
    while let Some(byte) = next_byte()? {
        if byte == b'\n' {
            // Exactly one CR is paired with the LF terminator and popped;
            // an interior CR stays content (CRCRLF keeps its first CR).
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            break;
        }
        // One byte past the cap is held in the spare slot ONLY if it is
        // the CR of a CRLF terminator; anything else overflows the cap.
        // The check runs BEFORE the push, so rejection never allocates,
        // while an exact-cap CR is still accepted (its LF pops it).
        if bytes.len() > SECRET_MAX_BYTES || (bytes.len() == SECRET_MAX_BYTES && byte != b'\r') {
            return Err(ShellError::SecretTooLong);
        }
        bytes.push(byte);
    }
    // At EOF the buffer must be within the cap: the spare byte is only
    // ever filled by a CR whose paired LF was consumed above (and popped
    // with it), so a 4097th byte surviving to here is oversize content.
    if bytes.len() > SECRET_MAX_BYTES {
        return Err(ShellError::SecretTooLong);
    }
    finish_session(bytes)
}

/// The ONE hidden-TTY line-termination rule (FOLLOW-UP 2), shared by the
/// real Unix adapter (byte-at-a-time, `u8` widened here), the real Windows
/// adapter (UTF-16 units, native `u16`) and the scripted test fake
/// (`tests/admin/shell/output.rs`): a terminal Enter arrives as CR, LF, or
/// CRLF, and the FIRST CR or LF ends the hidden line — the reader never
/// waits for an optional LF after a CR, so a single-CR console Enter
/// submits at once. Unlike the piped reader ([`read_line_bounded`], where
/// a CR paired with an LF is stripped), a hidden TTY password can never
/// contain a CR: the first CR is always a terminator, never content.
pub(crate) fn is_hidden_enter(unit: u16) -> bool {
    unit == u16::from(b'\r') || unit == u16::from(b'\n')
}

/// Shared final validation for a finished line (cross-sibling helper the
/// platform readers use too): empty is refused (BSTDIN precedent), non-UTF-8
/// is refused; validation borrows the bytes, so no unzeroized clone or temp
/// `String` is created, and the rejected bytes stay inside the zeroizing
/// buffer until it drops at the end of this function.
pub(super) fn finish_session(bytes: Zeroizing<Vec<u8>>) -> Result<Zeroizing<Vec<u8>>, ShellError> {
    if bytes.is_empty() {
        return Err(ShellError::InvalidSecret);
    }
    match std::str::from_utf8(&bytes) {
        // The borrowed bytes are moved into the returned zeroizing buffer
        // only after validation; no unzeroized copy or temp String exists.
        Ok(_) => Ok(bytes),
        Err(_) => Err(ShellError::InvalidSecret),
    }
}
