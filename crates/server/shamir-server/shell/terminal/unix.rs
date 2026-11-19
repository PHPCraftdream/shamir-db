//! Unix hidden-prompt entry: `/dev/tty`, `rustix::termios` (pinned 1.1.6),
//! restore on every exit path.
//!
//! Design: `docs/dev-artifacts/design/admin-shell.md` §3.2 (hidden TTY
//! prompt, echo disabled), §6 (terminal mode restored on every exit path,
//! no process exit from a guard). The prompt runs with `ECHO`, `ECHONL`,
//! `ICANON` and `ISIG` cleared *only for the duration of the prompt*: no
//! signal (Ctrl+C, Ctrl+Z, Ctrl+\) is generated while echo is off, so no
//! path can leave the terminal with echo off; byte 3 is recognized in the
//! raw stream and returned as [`ShellError::Interrupted`] after the restore.
//!
//! Overflow policy: once the 4096th content byte is seen the line can never
//! succeed, but returning immediately would restore echo in the middle of a
//! long paste (the rest of the paste would then be echoed) and leave the
//! unread paste tail pending for the next interactive read. Instead the
//! reader STAYS in no-echo mode and discards further bytes (never stored:
//! the accumulator is cleared, memory stays bounded) until the line's LF/CR
//! terminator; byte 3 still interrupts during the discard. Only then is
//! [`ShellError::SecretTooLong`] reported. The reader never joins a second
//! line and never consumes bytes past the first terminator, so a multi-line
//! paste cannot leak its tail into the command stream. A real OS read
//! failure is distinct from this policy: it is reported as
//! [`ShellError::SecretTerminal`] (the terminal itself failed) and cannot
//! wait for a terminator that may never come. A clean EOF mid-line remains
//! an incomplete-line refusal ([`ShellError::InvalidSecret`]).
//! Backspace (the tty's `VERASE` character) edits the line; LF and CR both
//! terminate it. `/dev/tty` is used, not stdin, so an explicitly requested
//! batch prompt works with redirected stdin.

use std::fs::{File, OpenOptions};

use rustix::termios::{self, LocalModes, OptionalActions, SpecialCodeIndex};
use zeroize::Zeroizing;

use crate::shell::output::ShellError;

use super::secret::{finish_session, HiddenLineSession, Terminal, SECRET_MAX_BYTES};

/// Ctrl+C arrives as byte 3 once `ISIG` is off (design: recognize byte 3 →
/// interrupt with restore).
const INTR_BYTE: u8 = 3;

/// Bounded retry count for the `Drop` restore backstop (a transient failure
/// of `tcsetattr` is retried; a persistent failure is dropped silently here
/// because `Drop` cannot report, and the explicit restore already returned
/// the typed error).
const RESTORE_BACKSTOP_ATTEMPTS: usize = 3;

/// The real Unix terminal.
pub(super) struct OsTerminal;

impl Terminal for OsTerminal {
    fn begin_hidden_read(&self) -> Result<Box<dyn HiddenLineSession + '_>, ShellError> {
        Ok(Box::new(UnixSession {
            guard: EchoOffGuard::acquire()?,
        }))
    }
}

/// One acquired no-echo session; the guard's `Drop` is the restore backstop
/// for every path, and `restore` is the explicit, error-reporting step.
struct UnixSession {
    guard: EchoOffGuard,
}

impl HiddenLineSession for UnixSession {
    fn read_line(&mut self) -> Result<Zeroizing<Vec<u8>>, ShellError> {
        let erase = self.guard.erase_byte;
        read_hidden_raw(&self.guard.tty, erase)
    }

    fn restore(mut self: Box<Self>) -> Result<(), ShellError> {
        self.guard.restore()
    }
}

/// Saves the `/dev/tty` attributes, applies the no-echo prompt modes, and
/// restores the original attributes. `restore` runs explicitly after every
/// read; `Drop` is the backstop that guarantees the restore across EVERY
/// return path (panics included), never exits the process, and retries a
/// failed restore a bounded number of times.
struct EchoOffGuard {
    tty: File,
    saved: rustix::termios::Termios,
    /// The tty's erase character, honored while canonical mode is off.
    erase_byte: u8,
    restored: bool,
}

impl EchoOffGuard {
    fn acquire() -> Result<Self, ShellError> {
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(|_| ShellError::SecretTerminal)?;
        let saved = termios::tcgetattr(&tty).map_err(|_| ShellError::SecretTerminal)?;
        let erase_byte = saved.special_codes[SpecialCodeIndex::VERASE];
        let mut prompt_modes = saved.clone();
        prompt_modes.local_modes &=
            !(LocalModes::ECHO | LocalModes::ECHONL | LocalModes::ICANON | LocalModes::ISIG);
        prompt_modes.special_codes[SpecialCodeIndex::VMIN] = 1;
        prompt_modes.special_codes[SpecialCodeIndex::VTIME] = 0;
        termios::tcsetattr(&tty, OptionalActions::Drain, &prompt_modes)
            .map_err(|_| ShellError::SecretTerminal)?;
        Ok(Self {
            tty,
            saved,
            erase_byte,
            restored: false,
        })
    }

    /// Explicit restore; marks the guard restored only on success so `Drop`
    /// retries a failed attempt.
    fn restore(&mut self) -> Result<(), ShellError> {
        if termios::tcsetattr(&self.tty, OptionalActions::Drain, &self.saved).is_ok() {
            self.restored = true;
            Ok(())
        } else {
            Err(ShellError::SecretTerminal)
        }
    }
}

impl Drop for EchoOffGuard {
    fn drop(&mut self) {
        if self.restored {
            return;
        }
        // Backstop across every return path: bounded retries, no
        // `process::exit` here (design §6). The typed failure is reported by
        // the explicit `restore`; a backstop-only path (panic between
        // disable and restore) cannot report anything.
        for _ in 0..RESTORE_BACKSTOP_ATTEMPTS {
            if self.restore().is_ok() {
                break;
            }
        }
    }
}

/// Raw byte-at-a-time read while canonical mode is off: manual erase
/// handling, LF/CR termination, byte 3 interrupt, 4096-byte cap. An
/// over-limit line is discarded (no-echo kept) up to its terminator and
/// then reported as `SecretTooLong` — see the module overflow policy.
/// The accumulator and the read scratch are zeroized on every path,
/// including the early returns (interrupt, overflow, EOF, failed read).
/// Restore happens in the guard's `Drop` after this returns.
fn read_hidden_raw(tty: &File, erase_byte: u8) -> Result<Zeroizing<Vec<u8>>, ShellError> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(SECRET_MAX_BYTES));
    let mut chunk = Zeroizing::new([0u8; 64]);
    // Set once the cap is exceeded: the line is doomed; further bytes are
    // discarded (never stored) until the LF/CR terminator.
    let mut over_limit = false;
    loop {
        // An OS read failure is a terminal fault, not a secret policy
        // violation: it cannot wait for a terminator that may never arrive.
        let read = rustix::io::read(tty, &mut chunk[..]).map_err(|_| ShellError::SecretTerminal)?;
        if read == 0 {
            // Clean EOF mid-line: incomplete input, typed refusal.
            return Err(ShellError::InvalidSecret);
        }
        for &byte in &chunk[..read] {
            match byte {
                INTR_BYTE => return Err(ShellError::Interrupted),
                b'\n' | b'\r' => {
                    if over_limit {
                        return Err(ShellError::SecretTooLong);
                    }
                    return finish_session(bytes);
                }
                b if b == erase_byte => {
                    if !over_limit {
                        bytes.pop();
                    }
                }
                _ => {
                    if over_limit {
                        // Discard: bounded-memory drain, nothing stored.
                        continue;
                    }
                    if bytes.len() == SECRET_MAX_BYTES {
                        over_limit = true;
                        bytes.clear();
                        continue;
                    }
                    bytes.push(byte);
                }
            }
        }
    }
}
