//! The shell entry points and the two run modes (design
//! `docs/dev-artifacts/design/admin-shell.md` §§4.1, §4.3, §5).
//!
//! Batch mode is exactly ONE argv command after a literal `--`; once it
//! parses, every failure emits one JSON Lines v1 error envelope, earlier
//! refusals write a fixed line to stderr only. Interactive mode is a bounded
//! line reader requiring stdin and stdout to be a TTY; every clap failure
//! maps to the fixed [`ShellError::Usage`] refusal (clap diagnostics could
//! echo argv; no raw argv/server text is ever surfaced); secrets on argv are
//! refused before parsing. The connection is persistent and never silently
//! re-established: after any dial/request failure, and on every failed
//! explicit `connect` (incl. auth refusals), a sticky sentinel makes further
//! non-`connect` commands refuse until an explicit `connect` succeeds (a
//! consumed secret source is never re-read). Every awaited request is
//! bounded by [`crate::shell::connect::REQUEST_TIMEOUT`]; Ctrl+C maps to
//! [`ShellError::Interrupted`]. For the in-package `tests/admin/` acceptance
//! (no TTY in CI), [`run_scripted`] drives the SAME loop from injected
//! input/stderr/terminal/profile root; the production interactive arm
//! delegates to the same entry with the real handles and `None` profile
//! root, and stdout stays silent in it.

use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal, Read, Write};
use std::iter;
use std::path::Path;

use clap::Parser as ClapParser;

use super::dispatch::dispatch;
use super::grammar::{command_id, CommandArgs, Mode, ShellArgs};
use super::input::{read_line_bounded, ReadLine};
use super::session::{Ctx, Session};
use crate::shell::output::{self, ShellError};
use crate::shell::terminal;
use crate::shell::terminal::secret::Terminal;

/// Fixed interactive help (stderr only; never argv-derived).
const INTERACTIVE_HELP: &str = "commands:\n  connect [NAME] [--auth scram|token]\n  ping\n  profile list | profile show NAME\n  db list\n  repo list | repo describe REPO\n  table list REPO | table describe REPO TABLE | table schema REPO TABLE\n  index list REPO TABLE\n  function list [FOLDER]\n  status\n  version show\n  replication subscriptions\n  data count DATABASE REPO TABLE | data head DATABASE REPO TABLE N | data get DATABASE REPO TABLE KEY\n  quit | exit\nsecret source flags: --secret-stdin | --secret-fd N (Unix only) | --secret-env NAME | --secret-prompt\ncaution: --secret-env NAME accepts only SHAMIR_SHELL_PASSWORD for SCRAM or SHAMIR_SHELL_TOKEN for token auth; an environment variable's value can be visible to other processes on the host and to shell history, so prefer --secret-stdin or the no-echo --secret-prompt. Profiles are stored in the profile directory (see `profile list`); the pin is never displayed.\n";

/// Shell entry point; argv after the leading `shell` token. Returns the
/// design §5 exit code. The locks are taken up front (locking is
/// side-effect-free); the §5 TTY gate still runs inside [`run_scripted`]
/// only after the policy refusals, exactly as before.
pub fn run(args: impl IntoIterator<Item = OsString>) -> u8 {
    let stdin = io::stdin();
    let stderr = io::stderr();
    let mut input = stdin.lock();
    let mut stderr = stderr.lock();
    run_scripted(
        args,
        &mut input,
        &mut stderr,
        terminal::secret::os_terminal(),
        None,
    )
}

/// The ONE dispatch implementation (design §5): argv handling (secret
/// refusal, batch split, fixed help), option parsing, runtime build, policy
/// refusals, then batch or interactive over the context handles. BOTH
/// callers land here, so the seam the `tests/admin/` acceptance target
/// drives is literally the production code path: the live dispatch
/// ([`run`]) passes `profile_root: None` with the real stdin/stderr locks
/// and OS terminal (the operator profile directory still resolves lazily
/// inside `connect::load`, and the TTY gate runs after the policy
/// refusals, exactly as before), while the no-TTY acceptance target passes
/// injected `input`/`stderr`/`terminal` and an explicit `profile_root`
/// (`Some`), which additionally refuses batch mode after `--` and never
/// reads the process-global environment.
pub fn run_scripted(
    args: impl IntoIterator<Item = OsString>,
    input: &mut dyn Read,
    stderr: &mut dyn Write,
    terminal: &dyn Terminal,
    profile_root: Option<&Path>,
) -> u8 {
    let live = profile_root.is_none();
    let argv: Vec<OsString> = args.into_iter().collect();
    if let Some(code) = refuse_argv_secret(&argv) {
        return code;
    }
    let (option_tokens, command_tokens) = split_batch(&argv);
    // Fixed help through the caller's writer (either spelling in the live
    // dispatch; the scripted seam never treats a bare `help` command as
    // help). Never argv-derived text, no stdout record.
    let help_requested = option_tokens.iter().any(|arg| arg == "--help")
        || (live
            && matches!(command_tokens.as_deref(), Some([one]) if *one == *OsStr::new("help")));
    if help_requested {
        let _ = stderr.write_all(INTERACTIVE_HELP.as_bytes());
        let _ = stderr.flush();
        return 0;
    }
    if !live && command_tokens.is_some() {
        // Scripted seam: batch mode after `--` is refused outright.
        return pre_refuse(ShellError::Usage);
    }
    let shell = match <ShellArgs as ClapParser>::try_parse_from(
        iter::once(OsString::from("shell")).chain(option_tokens.iter().cloned()),
    ) {
        Ok(shell) => shell,
        // clap's error text could reflect argv; only the fixed refusal is
        // ever surfaced.
        Err(_) => return pre_refuse(ShellError::Usage),
    };
    // Exactly one runtime, built before any policy refusal (§5 ordering).
    // Interactive mode needs a runtime worker: the loop below blocks on
    // synchronous terminal input OUTSIDE `block_on`, and only a worker
    // thread keeps the SDK's spawned tasks (the WebSocket reader answering
    // server pings) running while the operator is at the prompt.
    let runtime = match build_runtime(command_tokens.is_none()) {
        Ok(runtime) => runtime,
        Err(err) => return pre_refuse(err),
    };
    match command_tokens {
        Some(tokens) => {
            let mut ctx = Ctx {
                input,
                stderr,
                terminal,
                profile_root: None,
            };
            run_batch(&shell, tokens, &runtime, &mut ctx)
        }
        None => {
            // Interactive policy refusals stay stderr-only and precede the
            // TTY gate (§5 ordering; shared by live and scripted callers).
            if shell.resume {
                return pre_refuse(ShellError::Resume);
            }
            if shell.output_version.is_some_and(|version| version != 1) {
                return pre_refuse(ShellError::OutputVersion);
            }
            // The live TTY gate runs only here, after the refusals; the
            // scripted caller has no gate (injected handles, CI has no TTY).
            if live && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
                return pre_refuse(ShellError::MissingTty);
            }
            let mut ctx = Ctx {
                input,
                stderr,
                terminal,
                profile_root,
            };
            run_interactive(&shell, &runtime, &mut ctx)
        }
    }
}

/// Refuses any secret-bearing argv flag (bare or `=` form) before parsing.
fn refuse_argv_secret(argv: &[OsString]) -> Option<u8> {
    for arg in argv {
        let text = arg.to_string_lossy();
        if text == "--password"
            || text == "--token"
            || text.starts_with("--password=")
            || text.starts_with("--token=")
        {
            return Some(pre_refuse(ShellError::ArgvSecret));
        }
    }
    None
}

/// Splits argv at the first literal `--` (options / batch command).
fn split_batch(argv: &[OsString]) -> (Vec<OsString>, Option<Vec<OsString>>) {
    let separator = OsStr::new("--");
    match argv.iter().position(|arg| arg == separator) {
        Some(position) => (
            argv[..position].to_vec(),
            Some(argv[position + 1..].to_vec()),
        ),
        None => (argv.to_vec(), None),
    }
}

/// Fixed pre-parse refusal: one fixed stderr line, no stdout record.
fn pre_refuse(err: ShellError) -> u8 {
    let mut stderr = io::stderr();
    let _ = writeln!(stderr, "error: {} {}", err.code(), err.message());
    let _ = stderr.flush();
    err.exit()
}

/// Interactive mode needs ONE runtime worker: the loop below blocks on
/// synchronous terminal input OUTSIDE `block_on`, and only a runtime worker
/// keeps the SDK's spawned tasks (the WebSocket reader answering server
/// pings) running while the operator is at the prompt. Batch mode keeps the
/// single-command current-thread runtime unchanged.
fn build_runtime(interactive: bool) -> Result<tokio::runtime::Runtime, ShellError> {
    if !interactive {
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ShellError::Connection);
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|_| ShellError::Connection)
}

/// Batch mode: one parsed command, one JSON Lines v1 record.
fn run_batch(
    shell: &ShellArgs,
    tokens: Vec<OsString>,
    runtime: &tokio::runtime::Runtime,
    ctx: &mut Ctx<'_>,
) -> u8 {
    let command = match <CommandArgs as ClapParser>::try_parse_from(
        iter::once(OsString::from("shell")).chain(tokens),
    ) {
        Ok(args) => args.command,
        Err(_) => return pre_refuse(ShellError::Usage),
    };
    let id = command_id(&command);
    if shell.resume {
        return output::emit(true, id, None, Err(ShellError::Resume));
    }
    if shell.output_version.is_some_and(|version| version != 1) {
        return output::emit(true, id, None, Err(ShellError::OutputVersion));
    }
    let mut session: Option<Session> = None;
    let (id, op, result) = dispatch(shell, Mode::Batch, command, &mut session, runtime, ctx);
    output::emit(true, id, op, result)
}

/// The real interactive loop shared by production and the scripted entry.
fn run_interactive(shell: &ShellArgs, runtime: &tokio::runtime::Runtime, ctx: &mut Ctx<'_>) -> u8 {
    let prompt = if shell.quiet { "" } else { "> " };
    let mut session: Option<Session> = None;
    loop {
        if !prompt.is_empty() {
            let _ = ctx.stderr.write_all(prompt.as_bytes());
            let _ = ctx.stderr.flush();
        }
        let line = match read_line_bounded(&mut *ctx.input) {
            ReadLine::Line(line) => line,
            ReadLine::Eof => return 0,
            // Read error: emit once and stop (looping would spin).
            ReadLine::Terminal => {
                return output::emit_human(&mut *ctx.stderr, "shell", Err(ShellError::Usage));
            }
            ReadLine::Invalid => continue,
        };
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        match tokens[0] {
            "quit" | "exit" => return 0,
            "help" => {
                ctx.write_fixed(INTERACTIVE_HELP);
                continue;
            }
            _ => {}
        }
        let command = match <CommandArgs as ClapParser>::try_parse_from(
            iter::once("shell").chain(tokens.iter().copied()),
        ) {
            Ok(args) => args.command,
            Err(_) => {
                if tokens[0] == "connect" {
                    let auth = session.as_ref().map_or(shell.auth, |state| state.auth);
                    session = Some(Session { auth, live: None });
                }
                output::emit_human(&mut *ctx.stderr, "shell", Err(ShellError::Usage));
                continue;
            }
        };
        let (id, _, result) = dispatch(
            shell,
            Mode::Interactive,
            command,
            &mut session,
            runtime,
            ctx,
        );
        let connection_lost = matches!(
            result,
            Err(ShellError::Connection
                | ShellError::Timeout
                | ShellError::HeadWallTime
                | ShellError::Interrupted,)
        );
        if let Some(session) = session.as_mut() {
            if connection_lost {
                session.live = None;
            }
        }
        // One human record per command; stdout stays silent. Ctrl+C exits
        // 130 instead of falling back into the input loop.
        let interrupted = matches!(result, Err(ShellError::Interrupted));
        let exit = output::emit_human(&mut *ctx.stderr, id, result);
        if interrupted {
            return exit;
        }
    }
}
