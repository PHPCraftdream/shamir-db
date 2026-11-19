//! Windows hidden-prompt entry: `CONIN$` console handle, `SetConsoleMode`,
//! UTF-16 console reads, restore on every exit path.
//!
//! Design: `docs/dev-artifacts/design/admin-shell.md` §3.2 (hidden prompt),
//! §6 (restore on every exit path, no process exit from a guard). The input
//! handle is an OWNED handle opened on `CONIN$` (via `std::fs::OpenOptions`),
//! so the prompt works even when stdin is redirected but a console is still
//! attached; the handle is never duplicated or closed by hand — the owning
//! `File`'s drop closes it exactly once. The read goes through `ReadConsoleW`
//! so the input is Unicode regardless of the console code page; the UTF-16
//! scratch is zeroized on every path and the decoded units are folded
//! incrementally into a capacity-bounded zeroizing UTF-8 buffer (no plain
//! `String`, no reallocation past the cap). With `ENABLE_PROCESSED_INPUT`
//! off no Ctrl+C event is generated at all during the prompt — the key
//! arrives as the character `0x03` and is returned as
//! [`ShellError::Interrupted`] after the restore, so Ctrl+C can never kill
//! the process with echo left off.
//!
//! Failure policy (kept until end of line): an over-limit line or a
//! malformed UTF-16 unit (lone surrogate) makes the line doomed, but
//! returning immediately would restore echo mid-paste and leave the unread
//! paste tail pending for the next interactive read. The FIRST such policy
//! failure is remembered; the reader stays in no-echo mode, discards all
//! further decoded input (nothing stored; memory stays bounded) and keeps
//! draining until the line's CR/LF terminator, only then reporting the
//! remembered error. `0x03` still interrupts during the drain. The reader
//! never joins a second line and never reads past the terminator, so a
//! multi-line paste cannot leak its tail into the command stream. A real
//! OS read failure (`ReadConsoleW` returning zero) is distinct from policy:
//! it is reported immediately as [`ShellError::SecretTerminal`] — the
//! terminal itself failed and a terminator may never come. Termination
//! follows the ONE shared TTY framing rule (`secret::is_hidden_enter`):
//! Enter arrives as CR, LF, or CRLF and the FIRST CR or LF ends the line —
//! the reader never waits for an optional LF after a CR, so a native
//! console Enter that delivers a single CR submits the line at once.
//! Unlike the piped reader, a CR is always a terminator here, never
//! password content. No flush of the
//! console input buffer after the terminator is implemented: flushing would
//! require an unchecked `FlushConsoleInputBuffer` call, which this module
//! deliberately avoids. The exact limitation: bytes the OS delivered in the
//! same read burst AFTER the terminating CR/LF (up to 16 units per read)
//! are dropped here, but any further units the console still holds for the
//! paste stay queued in the console input buffer and will be delivered to
//! the NEXT reader (e.g. the interactive line reader) — an operator who
//! keeps typing past a rejected oversized line must expect that tail to
//! surface later; it can never appear as part of the secret or of THIS
//! command.

use std::fs::{File, OpenOptions};
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{
    GetConsoleMode, ReadConsoleW, SetConsoleMode, CONSOLE_MODE, ENABLE_ECHO_INPUT,
    ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
};
use zeroize::Zeroizing;

use crate::shell::output::ShellError;

use super::secret::{
    finish_session, is_hidden_enter, HiddenLineSession, Terminal, SECRET_MAX_BYTES,
};

/// Ctrl+C arrives as this character once `ENABLE_PROCESSED_INPUT` is off.
const INTR_CHAR: u16 = 3;
/// Backspace arrives as this character once `ENABLE_LINE_INPUT` is off.
const ERASE_CHAR: u16 = 8;

/// Bounded retry count for the `Drop` restore backstop (see `unix.rs`).
const RESTORE_BACKSTOP_ATTEMPTS: usize = 3;

/// The real Windows terminal.
pub(super) struct OsTerminal;

impl Terminal for OsTerminal {
    fn begin_hidden_read(&self) -> Result<Box<dyn HiddenLineSession + '_>, ShellError> {
        Ok(Box::new(WindowsSession {
            guard: ConsoleEchoOffGuard::acquire()?,
        }))
    }
}

/// One acquired no-echo session; the guard's `Drop` is the restore backstop
/// for every path, and `restore` is the explicit, error-reporting step.
struct WindowsSession {
    guard: ConsoleEchoOffGuard,
}

impl HiddenLineSession for WindowsSession {
    fn read_line(&mut self) -> Result<Zeroizing<Vec<u8>>, ShellError> {
        read_hidden_utf16(&self.guard)
    }

    fn restore(mut self: Box<Self>) -> Result<(), ShellError> {
        self.guard.restore()
    }
}

/// Saves the input console mode, clears the echo/line/processed flags, and
/// restores the original mode. `restore` runs explicitly after every read;
/// `Drop` is the backstop that guarantees the restore across EVERY return
/// path (panics included), never exits the process, and retries a failed
/// restore a bounded number of times.
struct ConsoleEchoOffGuard {
    /// The owned `CONIN$` handle; dropped (closed) exactly once by `File`.
    console_file: File,
    saved: CONSOLE_MODE,
    restored: bool,
}

impl ConsoleEchoOffGuard {
    fn acquire() -> Result<Self, ShellError> {
        // An owned handle on the attached console's input buffer; unlike
        // `GetStdHandle(STD_INPUT_HANDLE)` this stays a console handle even
        // when stdin itself is redirected (design: explicit batch prompt).
        let console_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("\\\\.\\CONIN$")
            .map_err(|_| ShellError::SecretTerminal)?;
        // The live handle of the owned file above; used only while the file
        // is alive and never closed by hand.
        let console = console_file.as_raw_handle() as HANDLE;
        if console.is_null() || console == INVALID_HANDLE_VALUE {
            return Err(ShellError::SecretTerminal);
        }
        let mut saved: CONSOLE_MODE = 0;
        // SAFETY: `console` is a live console input handle; `saved` is a
        // valid out-pointer for one `CONSOLE_MODE`.
        if unsafe { GetConsoleMode(console, &mut saved) } == 0 {
            return Err(ShellError::SecretTerminal);
        }
        let prompt_modes =
            saved & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT);
        // SAFETY: `console` is a live console input handle from above.
        if unsafe { SetConsoleMode(console, prompt_modes) } == 0 {
            return Err(ShellError::SecretTerminal);
        }
        Ok(Self {
            console_file,
            saved,
            restored: false,
        })
    }

    /// The live console input handle, borrowed from the owning `File`; it
    /// is never closed by hand and stays valid while the guard exists.
    fn console(&self) -> HANDLE {
        self.console_file.as_raw_handle() as HANDLE
    }

    /// Explicit restore; marks the guard restored only on success so `Drop`
    /// retries a failed attempt.
    fn restore(&mut self) -> Result<(), ShellError> {
        // SAFETY: `console` is a live console input handle; `saved` is the
        // mode captured before any modification.
        if unsafe { SetConsoleMode(self.console(), self.saved) } != 0 {
            self.restored = true;
            Ok(())
        } else {
            Err(ShellError::SecretTerminal)
        }
    }
}

impl Drop for ConsoleEchoOffGuard {
    fn drop(&mut self) {
        if self.restored {
            return;
        }
        // Backstop across every return path: bounded retries, no
        // `process::exit` here (design §6).
        for _ in 0..RESTORE_BACKSTOP_ATTEMPTS {
            if self.restore().is_ok() {
                break;
            }
        }
    }
}

/// UTF-16 console read with incremental UTF-16→UTF-8 folding, manual erase
/// handling, CR/LF termination, the `0x03` interrupt, and the 4096-byte cap
/// applied per encoded byte. Policy failures (cap, malformed UTF-16) are
/// remembered until the terminator — see the module failure policy. The
/// UTF-16 scratch and the accumulator are zeroized on every path, including
/// the early returns (interrupt, pending failure, failed read). Restore
/// happens in the guard's `Drop` after this returns.
fn read_hidden_utf16(guard: &ConsoleEchoOffGuard) -> Result<Zeroizing<Vec<u8>>, ShellError> {
    let console = guard.console();
    let mut bytes = Zeroizing::new(Vec::with_capacity(SECRET_MAX_BYTES));
    let mut chunk = Zeroizing::new([0u16; 16]);
    let mut pending_high: Option<u16> = None;
    // The FIRST policy failure (cap exceeded, malformed UTF-16, incomplete
    // surrogate at terminator): remembered until the line terminates, then
    // reported — see the module failure policy.
    let mut pending_error: Option<ShellError> = None;
    loop {
        let mut got: u32 = 0;
        // SAFETY: `chunk` is a valid UTF-16 buffer with room for one element
        // per requested unit (`got` is its out-pointer; the control record
        // is not needed and passed as null — verified windows-sys 0.61.2
        // signature: `(HANDLE, *mut c_void, u32, *mut u32,
        // *const CONSOLE_READCONSOLE_CONTROL) -> BOOL`).
        if unsafe {
            ReadConsoleW(
                console,
                chunk.as_mut_ptr().cast::<core::ffi::c_void>(),
                chunk.len() as u32,
                &mut got,
                core::ptr::null(),
            )
        } == 0
        {
            // OS read failure, distinct from secret policy: reported
            // immediately; no terminator can be waited for.
            return Err(ShellError::SecretTerminal);
        }
        if got == 0 {
            return Err(ShellError::InvalidSecret);
        }
        for &unit in &chunk[..got as usize] {
            if unit == INTR_CHAR {
                return Err(ShellError::Interrupted);
            }
            if is_hidden_enter(unit) {
                if let Some(err) = pending_error {
                    return Err(err);
                }
                if pending_high.is_some() {
                    return Err(ShellError::InvalidSecret);
                }
                return finish_session(bytes);
            }
            if pending_error.is_some() {
                // Draining a doomed line: discard everything until CR/LF.
                continue;
            }
            if unit == ERASE_CHAR {
                pop_char(&mut bytes);
                continue;
            }
            match decode_unit(unit, &mut pending_high) {
                Ok(Some(ch)) => {
                    if let Err(err) = push_char_capped(&mut bytes, ch) {
                        pending_error = Some(err);
                    }
                }
                Ok(None) => {}
                Err(err) => pending_error = Some(err),
            }
        }
    }
}

/// Fold one UTF-16 unit into the pending-surrogate state; returns a decoded
/// `char` when a full character (or a plain BMP unit) completed. A lone
/// surrogate is a typed refusal, never a silent replacement.
fn decode_unit(unit: u16, pending_high: &mut Option<u16>) -> Result<Option<char>, ShellError> {
    let is_high = (0xD800..0xDC00).contains(&unit);
    let is_low = (0xDC00..0xE000).contains(&unit);
    if is_high {
        if pending_high.is_some() {
            return Err(ShellError::InvalidSecret);
        }
        *pending_high = Some(unit);
        return Ok(None);
    }
    if is_low {
        let Some(high) = pending_high.take() else {
            return Err(ShellError::InvalidSecret);
        };
        let combined = 0x10000 + (((high - 0xD800) as u32) << 10) + ((unit - 0xDC00) as u32);
        return Ok(char::from_u32(combined));
    }
    if pending_high.take().is_some() {
        return Err(ShellError::InvalidSecret);
    }
    Ok(char::from_u32(unit as u32))
}

/// Append one character to the accumulator, enforcing the cap per encoded
/// byte so the buffer never grows past the bounded capacity.
fn push_char_capped(bytes: &mut Zeroizing<Vec<u8>>, ch: char) -> Result<(), ShellError> {
    let mut encoded = Zeroizing::new([0u8; 4]);
    for &byte in ch.encode_utf8(&mut encoded[..]).as_bytes() {
        if bytes.len() == SECRET_MAX_BYTES {
            return Err(ShellError::SecretTooLong);
        }
        bytes.push(byte);
    }
    Ok(())
}

/// Erase the last whole character (a trailing byte plus its UTF-8
/// continuation bytes), mirroring the terminal's backspace semantics.
fn pop_char(bytes: &mut Zeroizing<Vec<u8>>) {
    if bytes.pop().is_some() {
        while matches!(bytes.last(), Some(&b) if (0x80..0xC0).contains(&b)) {
            bytes.pop();
        }
    }
}
