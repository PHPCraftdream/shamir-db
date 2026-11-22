//! Deterministic regression for the interactive decline (design §3.2):
//! `run_interactive`'s sticky `declined` must survive a later `exit`/EOF.
//!
//! The scripted seam IS the real interactive loop (injected stdin, captured
//! stderr, injected terminal and injected profile root). The decline happens
//! BEFORE any profile load or dial, so the terminal stub below is never
//! asked for a hidden read; it refuses loudly if it ever is.

use std::ffi::OsString;
use std::io::Cursor;
use std::path::Path;

use crate::shell::output::ShellError;
use crate::shell::terminal::secret::{HiddenLineSession, Terminal};

/// Terminal stub: any hidden read is the typed `SecretTerminal` refusal.
struct RefusingTerminal;

impl Terminal for RefusingTerminal {
    fn begin_hidden_read(&self) -> Result<Box<dyn HiddenLineSession + '_>, ShellError> {
        Err(ShellError::SecretTerminal)
    }
}

/// One scripted interactive session against an explicit profile root;
/// returns the exit code and the captured stderr as text.
fn scripted(lines: &str, root: &Path) -> (u8, String) {
    let mut input = Cursor::new(lines.as_bytes().to_vec());
    let mut stderr: Vec<u8> = Vec::new();
    let terminal = RefusingTerminal;
    let code = crate::shell::parser::run::run_scripted(
        [
            OsString::from("-q"),
            OsString::from("--profile"),
            OsString::from("fixture"),
        ],
        &mut input,
        &mut stderr,
        &terminal,
        Some(root),
    );
    let stderr = String::from_utf8(stderr).expect("scripted stderr is UTF-8");
    (code, stderr)
}

/// An interactively declined `chmod` exits 6 and the refusal is STICKY: the
/// following `exit` cannot turn it into 0 (mutant: the old `return 0` on
/// `exit`/EOF). No profile named `fixture` exists in the root and the
/// terminal stub refuses every hidden read: the decline happens before both.
#[test]
fn interactive_decline_keeps_exit_six_through_exit() {
    let root = tempfile::tempdir().expect("temp profile root");
    let (code, stderr) = scripted("chmod mydb/main/tbl 0750\nno\nexit\n", root.path());
    assert_eq!(
        code, 6,
        "a declined confirmation must survive the later `exit` (sticky `declined`)"
    );
    assert!(
        stderr.contains("confirmation_required"),
        "the fixed confirmation code must be reported: {stderr}"
    );
    assert!(
        stderr.contains("type yes to confirm"),
        "the fixed interactive prompt must be written: {stderr}"
    );
}

/// The answer `yes` PASSES the confirmation, so the command moves on to
/// `establish_profile`. The injected root holds no profile named `fixture`,
/// so the load fails and the fixed human error block is rendered. The test
/// does not pin WHICH profile error (the load path is allowed to classify
/// it); it pins that the confirmation was PASSED (the prompt was issued and
/// `confirmation_required` is absent) and that no sticky decline exists at
/// EOF. Exit 2 (`connection_failed`) is NOT reached — the dial never happens
/// because the profile cannot be read — and the scripted loop reports the
/// per-command failure on stderr only, so the session status at EOF is 0
/// (the REPL's final status is the sticky decline alone; see
/// `run_interactive`).
#[test]
fn confirmed_yes_passes_confirmation_then_profile_load_fails() {
    let root = tempfile::tempdir().expect("temp profile root");
    let (code, stderr) = scripted("chmod mydb/main/tbl 0750\nyes\n", root.path());
    assert_eq!(
        code, 0,
        "the scripted loop ends at EOF; only a sticky decline changes the status"
    );
    assert!(
        stderr.contains("type yes to confirm"),
        "the confirmation must have been prompted: {stderr}"
    );
    assert!(
        !stderr.contains("confirmation_required"),
        "`yes` passes the confirmation: {stderr}"
    );
    assert!(
        stderr.contains("error\n") && stderr.contains("code"),
        "the failed profile load must render the fixed human error block: {stderr}"
    );
}
