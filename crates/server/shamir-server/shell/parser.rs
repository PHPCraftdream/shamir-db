//! The shell command parser and its two modes (design
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
//! bounded by [`super::connect::REQUEST_TIMEOUT`]; Ctrl+C maps to
//! [`ShellError::Interrupted`]. For the in-package `tests/admin/` acceptance
//! (no TTY in CI), [`run_scripted`] drives the SAME loop from injected
//! input/stderr/terminal/profile root; the production dispatch never uses
//! it and stdout stays silent in it.

use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::io::{self, IsTerminal, Read, Write};
use std::iter;
use std::path::Path;

use clap::Parser as ClapParser;
use serde_json::json;
use serde_json::Value as Json;
use shamir_client::Client;

use super::commands::Catalogue;
use super::connect;
use super::connect::Profile;
use super::output::{self, ShellError};
use super::terminal::secret::Terminal;
use super::terminal::{self, SecretSource};

/// Hard byte bound for one interactive input line.
const LINE_MAX_BYTES: usize = 8 * 1024;

/// Fixed interactive help (stderr only; never argv-derived).
const INTERACTIVE_HELP: &str = "commands:\n  connect [NAME] [--auth scram]\n  ping\n  profile list | profile show NAME\n  db list\n  repo list\n  table list REPO | table describe REPO TABLE | table schema REPO TABLE\n  index list REPO TABLE\n  function list [FOLDER]\n  quit | exit\nsecret source flags: --secret-stdin | --secret-fd N (Unix only) | --secret-env NAME | --secret-prompt\ncaution: --secret-env NAME accepts only the fixed variable SHAMIR_SHELL_PASSWORD; an environment variable's value can be visible to other processes on the host and to shell history, so prefer --secret-stdin or the no-echo --secret-prompt. Profiles are stored in the profile directory (see `profile list`); the pin is never displayed.\n";

/// Batch or interactive; interactive defaults the no-echo prompt source.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Batch,
    Interactive,
}

/// One dispatched outcome: command id, wire op, result.
type Outcome = (&'static str, Option<&'static str>, Result<Json, ShellError>);

/// Persistent interactive session; `dead` is the sticky refusal sentinel.
struct Session {
    profile: Option<Profile>,
    client: Option<Client>,
    dead: bool,
}

/// Injectable environment behind [`run_scripted`]: injected input,
/// stderr, terminal adapter and profile root (see the [`Ctx`] fields).
struct Ctx<'a> {
    input: &'a mut dyn Read,
    stderr: &'a mut dyn Write,
    terminal: &'a dyn Terminal,
    profile_root: Option<&'a Path>,
}

impl Ctx<'_> {
    /// One fixed, locally authored text block to the context's stderr.
    fn write_fixed(&mut self, text: &str) {
        let _ = self.stderr.write_all(text.as_bytes());
        let _ = self.stderr.flush();
    }

    /// Loads one profile through the injected root (or operator directory).
    fn load_profile(&self, name: &str) -> Result<Profile, ShellError> {
        match self.profile_root {
            Some(root) => connect::load_from(root, name),
            None => connect::load(name),
        }
    }
}

/// Shell-level options, parsed only from the argv before the literal `--`.
#[derive(ClapParser)]
#[command(
    name = "shamir-server shell",
    disable_help_flag = true,
    disable_version_flag = true
)]
struct ShellArgs {
    /// Connection profile name from the profile directory.
    #[arg(long, value_name = "NAME")]
    profile: Option<String>,

    /// Authentication kind. Only `scram` is supported in this stage.
    #[arg(long, value_parser = ["scram", "token"], value_name = "AUTH")]
    auth: Option<String>,

    /// Session resume is refused outright.
    #[arg(long)]
    resume: bool,

    /// Read the secret from standard input.
    #[arg(long)]
    secret_stdin: bool,

    /// Read the secret from file descriptor N (Unix only).
    #[arg(long, value_name = "N")]
    secret_fd: Option<u32>,

    /// Read the secret from the environment. The name must be exactly
    /// SHAMIR_SHELL_PASSWORD; any other name is a typed refusal.
    #[arg(long, value_name = "NAME")]
    secret_env: Option<String>,

    /// Read the secret with a no-echo terminal prompt (explicit; batch mode
    /// must always name the source).
    #[arg(long)]
    secret_prompt: bool,

    /// Quiet: suppress interactive diagnostics on stderr.
    #[arg(short = 'q', conflicts_with = "verbose")]
    quiet: bool,

    /// Verbose: extra informational lines on stderr only.
    #[arg(short = 'v')]
    verbose: bool,

    /// Disable color (PR1 emits no color).
    #[arg(long)]
    no_color: bool,

    /// Local assertion of the output schema version; only 1 is accepted.
    #[arg(long, value_name = "VERSION")]
    output_version: Option<u32>,
}

/// The single-command grammar wrapper (the command enum is a `Subcommand`).
#[derive(ClapParser)]
#[command(
    name = "shamir-server shell",
    disable_help_flag = true,
    disable_version_flag = true
)]
struct CommandArgs {
    #[command(subcommand)]
    command: Command,
}

/// The command grammar; no free-form tail, so extra argv tokens are usage refusals.
#[derive(clap::Subcommand)]
enum Command {
    /// Dial, verify the profile pin and authenticate with SCRAM.
    Connect {
        name: Option<String>,
        #[arg(long)]
        auth: Option<String>,
    },
    /// Round-trip the live connection.
    Ping,
    /// Local profile-directory commands (no secret is ever displayed).
    Profile {
        #[command(subcommand)]
        action: ProfileCommand,
    },
    Db {
        #[command(subcommand)]
        action: DbCommand,
    },
    Repo {
        #[command(subcommand)]
        action: RepoCommand,
    },
    Table {
        #[command(subcommand)]
        action: TableCommand,
    },
    Index {
        #[command(subcommand)]
        action: IndexCommand,
    },
    Function {
        #[command(subcommand)]
        action: FunctionCommand,
    },
}

#[derive(clap::Subcommand)]
enum ProfileCommand {
    /// List the profile names in the profile directory.
    List,
    /// Show one profile; the pin is reported as present/absent only.
    Show { name: String },
}

#[derive(clap::Subcommand)]
enum DbCommand {
    /// List databases.
    List,
}

#[derive(clap::Subcommand)]
enum RepoCommand {
    /// List repositories.
    List,
    /// PR2: intentionally unsupported in this stage (typed refusal).
    Describe { repo: Option<String> },
}

#[derive(clap::Subcommand)]
enum TableCommand {
    /// List tables of a repository.
    List { repo: String },
    /// Describe a table.
    Describe { repo: String, table: String },
    /// Show a table schema.
    Schema { repo: String, table: String },
}

#[derive(clap::Subcommand)]
enum IndexCommand {
    /// List indexes of a table.
    List { repo: String, table: String },
}

#[derive(clap::Subcommand)]
enum FunctionCommand {
    /// List functions, optionally restricted to a folder.
    List { folder: Option<String> },
}

/// Shell entry point; argv after the leading `shell` token. Returns the
/// design §5 exit code.
pub fn run(args: impl IntoIterator<Item = OsString>) -> u8 {
    run_inner(args)
}

/// Scripted-regression entry for the in-package `tests/admin/` target (no
/// TTY in CI): identical argv handling, parsing, policy refusals, profile
/// loading, secret resolution, SCRAM dialing and dispatch as [`run`], with
/// `input`/`stderr`/`terminal`/`profile_root` injected. No production TTY
/// gate; batch mode after `--` is refused; no global environment is read;
/// never used by the production dispatch.
///
/// `#[cfg(test)] pub(crate)`, not plain `pub`: no production caller uses it,
/// so `cfg(test)` keeps it out of the production build entirely; the binary's
/// own `#[cfg(test)]` build and the `tests/admin/` integration target both
/// reference it, keeping it alive under `-D warnings`.
#[cfg(test)]
pub(crate) fn run_scripted(
    args: impl IntoIterator<Item = OsString>,
    input: &mut dyn Read,
    stderr: &mut dyn Write,
    terminal: &dyn Terminal,
    profile_root: &Path,
) -> u8 {
    let argv: Vec<OsString> = args.into_iter().collect();
    if let Some(code) = refuse_argv_secret(&argv) {
        return code;
    }
    let (option_tokens, command_tokens) = split_batch(&argv);
    // Fixed `--help` through the injected writer so the capture sees it.
    if option_tokens.iter().any(|arg| arg == "--help") {
        let _ = stderr.write_all(INTERACTIVE_HELP.as_bytes());
        let _ = stderr.flush();
        return 0;
    }
    if command_tokens.is_some() {
        return pre_refuse(ShellError::Usage);
    }
    let shell = match <ShellArgs as ClapParser>::try_parse_from(
        iter::once(OsString::from("shell")).chain(option_tokens.iter().cloned()),
    ) {
        Ok(shell) => shell,
        Err(_) => return pre_refuse(ShellError::Usage),
    };
    let runtime = match build_runtime() {
        Ok(runtime) => runtime,
        Err(err) => return pre_refuse(err),
    };
    if shell.resume {
        return pre_refuse(ShellError::Resume);
    }
    if shell.auth.as_deref() == Some("token") {
        return pre_refuse(ShellError::Token);
    }
    if shell.output_version.is_some_and(|version| version != 1) {
        return pre_refuse(ShellError::OutputVersion);
    }
    let mut ctx = Ctx {
        input,
        stderr,
        terminal,
        profile_root: Some(profile_root),
    };
    run_interactive(&shell, &runtime, &mut ctx)
}

fn run_inner(args: impl IntoIterator<Item = OsString>) -> u8 {
    let argv: Vec<OsString> = args.into_iter().collect();
    if let Some(code) = refuse_argv_secret(&argv) {
        return code;
    }
    let (option_tokens, command_tokens) = split_batch(&argv);
    let help_requested = option_tokens.iter().any(|arg| arg == "--help")
        || matches!(command_tokens.as_deref(), Some([one]) if *one == *OsStr::new("help"));
    if help_requested {
        write_fixed_stderr(INTERACTIVE_HELP);
        return 0;
    }
    let shell = match <ShellArgs as ClapParser>::try_parse_from(
        iter::once(OsString::from("shell")).chain(option_tokens.iter().cloned()),
    ) {
        Ok(shell) => shell,
        // clap's error text could reflect argv; only the fixed refusal is
        // ever surfaced.
        Err(_) => return pre_refuse(ShellError::Usage),
    };
    let runtime = match build_runtime() {
        Ok(runtime) => runtime,
        Err(err) => return pre_refuse(err),
    };
    match command_tokens {
        Some(tokens) => {
            // Stack-owned locks, borrowed by the context for the dispatch only.
            let stdin = io::stdin();
            let stderr = io::stderr();
            let mut input = stdin.lock();
            let mut stderr = stderr.lock();
            let mut ctx = Ctx {
                input: &mut input,
                stderr: &mut stderr,
                terminal: terminal::secret::os_terminal(),
                profile_root: None,
            };
            run_batch(&shell, tokens, &runtime, &mut ctx)
        }
        None => {
            // Interactive policy refusals stay stderr-only.
            if shell.resume {
                return pre_refuse(ShellError::Resume);
            }
            if shell.auth.as_deref() == Some("token") {
                return pre_refuse(ShellError::Token);
            }
            if shell.output_version.is_some_and(|version| version != 1) {
                return pre_refuse(ShellError::OutputVersion);
            }
            // The TTY gate runs before any lock is taken.
            if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
                return pre_refuse(ShellError::MissingTty);
            }
            let stdin = io::stdin();
            let stderr = io::stderr();
            let mut input = stdin.lock();
            let mut stderr = stderr.lock();
            let mut ctx = Ctx {
                input: &mut input,
                stderr: &mut stderr,
                terminal: terminal::secret::os_terminal(),
                profile_root: None,
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

fn build_runtime() -> Result<tokio::runtime::Runtime, ShellError> {
    tokio::runtime::Builder::new_current_thread()
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
    if shell.auth.as_deref() == Some("token") {
        return output::emit(true, id, None, Err(ShellError::Token));
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
            Err(ShellError::Connection | ShellError::Timeout | ShellError::Interrupted)
        );
        if let Some(session) = session.as_mut() {
            if connection_lost {
                session.client = None;
                session.dead = true;
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

/// Writes one fixed, locally authored text block to stderr.
fn write_fixed_stderr(text: &str) {
    let mut stderr = io::stderr();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.flush();
}

/// Stable space-separated command id for the JSON v1 `command` field.
fn command_id(command: &Command) -> &'static str {
    match command {
        Command::Connect { .. } => "connect",
        Command::Ping => "ping",
        Command::Profile { action } => match action {
            ProfileCommand::List => "profile list",
            ProfileCommand::Show { .. } => "profile show",
        },
        Command::Db {
            action: DbCommand::List,
        } => "db list",
        Command::Repo { action } => match action {
            RepoCommand::List => "repo list",
            RepoCommand::Describe { .. } => "repo describe",
        },
        Command::Table { action } => match action {
            TableCommand::List { .. } => "table list",
            TableCommand::Describe { .. } => "table describe",
            TableCommand::Schema { .. } => "table schema",
        },
        Command::Index {
            action: IndexCommand::List { .. },
        } => "index list",
        Command::Function {
            action: FunctionCommand::List { .. },
        } => "function list",
    }
}

/// One outcome of the bounded interactive line reader.
enum ReadLine {
    /// A complete, valid line (terminator stripped).
    Line(String),
    /// Clean EOF at the start of a line.
    Eof,
    /// stdin read error: terminal for the loop (never retried).
    Terminal,
    /// Invalid physical line — control byte, lone CR, invalid UTF-8 or over
    /// [`LINE_MAX_BYTES`]; the line was drained, so no second command surfaces.
    Invalid,
}

/// Reads one physical line with a hard byte bound. A CR terminates only as
/// CRLF; a lone CR is an invalid, drained line.
fn read_line_bounded(reader: &mut dyn Read) -> ReadLine {
    let mut buffer: Vec<u8> = Vec::new();
    let mut byte_buf = [0u8; 1];
    let mut over_bound = false;
    loop {
        let byte = match reader.read(&mut byte_buf) {
            Ok(0) => {
                return if over_bound {
                    ReadLine::Invalid
                } else if buffer.is_empty() {
                    ReadLine::Eof
                } else {
                    finish_line(&buffer)
                };
            }
            Ok(n) => {
                debug_assert!(n == 1);
                byte_buf[0]
            }
            Err(_) => return ReadLine::Terminal,
        };
        if byte == b'\n' {
            return if over_bound {
                ReadLine::Invalid
            } else {
                finish_line(&buffer)
            };
        }
        if byte == b'\r' {
            // CR terminates only when immediately followed by LF; CR at EOF
            // ends the partial line; a lone CR is refused and drained.
            match reader.read(&mut byte_buf) {
                Ok(n) if n == 0 || byte_buf[0] == b'\n' => {
                    return if over_bound {
                        ReadLine::Invalid
                    } else {
                        finish_line(&buffer)
                    };
                }
                Ok(_) => {
                    drain_to_lf(reader, Some(byte_buf[0]));
                    return ReadLine::Invalid;
                }
                Err(_) => return ReadLine::Terminal,
            }
        }
        // Other control bytes are refused: drain and report invalid.
        if byte < 0x20 || byte == 0x7f {
            drain_to_lf(reader, None);
            return ReadLine::Invalid;
        }
        if buffer.len() >= LINE_MAX_BYTES {
            over_bound = true;
            continue;
        }
        buffer.push(byte);
    }
}

/// Consumes bytes up to and including the next LF (or EOF/error), so an
/// invalid physical line leaves nothing behind; no unbounded allocation.
fn drain_to_lf(reader: &mut dyn Read, first: Option<u8>) {
    if first == Some(b'\n') {
        return;
    }
    let mut byte_buf = [0u8; 1];
    loop {
        match reader.read(&mut byte_buf) {
            Ok(0) | Err(_) => return,
            Ok(_) if byte_buf[0] == b'\n' => return,
            Ok(_) => {}
        }
    }
}

/// Decodes one bounded line; invalid UTF-8 is a refusal, never lossy text.
fn finish_line(buffer: &[u8]) -> ReadLine {
    match String::from_utf8(buffer.to_vec()) {
        Ok(line) => ReadLine::Line(line),
        Err(_) => ReadLine::Invalid,
    }
}

/// Dispatches one parsed command; batch callers pass a fresh `None` session.
fn dispatch(
    shell: &ShellArgs,
    mode: Mode,
    command: Command,
    session: &mut Option<Session>,
    runtime: &tokio::runtime::Runtime,
    ctx: &mut Ctx<'_>,
) -> Outcome {
    match command {
        Command::Connect { name, auth } => {
            // `--auth`: `scram` (or default) is the only supported kind.
            let outcome = match auth.as_deref() {
                None | Some("scram") => {
                    establish_profile(shell, mode, name.as_deref(), runtime, false, ctx)
                }
                Some("token") => Err(ShellError::Token),
                Some("browser") | Some("http-login") => Err(ShellError::Deferred),
                Some("resume") => Err(ShellError::Resume),
                Some(_) => Err(ShellError::Usage),
            };
            match outcome {
                Ok((profile, client)) => {
                    *session = Some(Session {
                        profile: Some(profile),
                        client: Some(client),
                        dead: false,
                    });
                    note(shell.verbose, "connected");
                    ("connect", None, Ok(json!({ "connected": true })))
                }
                // EVERY failed explicit connect — dial failure and every
                // auth refusal alike — sticks the disconnected sentinel, so
                // `with_session` never implicitly re-dials (re-reading a
                // consumed secret source); `connect` itself never consults
                // the sentinel, so an explicit retry stays possible.
                Err(err) => {
                    *session = Some(Session {
                        profile: None,
                        client: None,
                        dead: true,
                    });
                    ("connect", None, Err(err))
                }
            }
        }
        Command::Ping => {
            let outcome =
                with_session(shell, mode, session, runtime, false, ctx).and_then(|(_, client)| {
                    runtime.block_on(guarded(async {
                        client.ping().await.map_err(connect::request_error)
                    }))
                });
            (
                "ping",
                Some("Ping"),
                outcome.map(|()| json!({ "ok": true })),
            )
        }
        Command::Profile { action } => profile_outcome(shell, action, ctx),
        Command::Db {
            action: DbCommand::List,
        } => catalogue(shell, mode, session, runtime, ctx, Catalogue::Databases),
        Command::Repo { action } => match action {
            RepoCommand::List => catalogue(shell, mode, session, runtime, ctx, Catalogue::Repos),
            RepoCommand::Describe { .. } => ("repo describe", None, Err(ShellError::Unsupported)),
        },
        Command::Table { action } => match action {
            TableCommand::List { repo } => catalogue(
                shell,
                mode,
                session,
                runtime,
                ctx,
                Catalogue::Tables { repo },
            ),
            TableCommand::Describe { repo, table } => catalogue(
                shell,
                mode,
                session,
                runtime,
                ctx,
                Catalogue::Describe { repo, table },
            ),
            TableCommand::Schema { repo, table } => catalogue(
                shell,
                mode,
                session,
                runtime,
                ctx,
                Catalogue::Schema { repo, table },
            ),
        },
        Command::Index {
            action: IndexCommand::List { repo, table },
        } => catalogue(
            shell,
            mode,
            session,
            runtime,
            ctx,
            Catalogue::Indexes { repo, table },
        ),
        Command::Function {
            action: FunctionCommand::List { folder },
        } => catalogue(
            shell,
            mode,
            session,
            runtime,
            ctx,
            Catalogue::Functions { folder },
        ),
    }
}

/// Catalogue command; the `database` floor is validated in [`with_session`].
fn catalogue(
    shell: &ShellArgs,
    mode: Mode,
    session: &mut Option<Session>,
    runtime: &tokio::runtime::Runtime,
    ctx: &mut Ctx<'_>,
    which: Catalogue,
) -> Outcome {
    let (id, op) = (which.id(), Some(which.op()));
    let outcome =
        with_session(shell, mode, session, runtime, true, ctx).and_then(|(profile, client)| {
            let database = profile.database.as_deref().ok_or(ShellError::Usage)?;
            runtime.block_on(guarded(async { which.execute(client, database).await }))
        });
    (id, op, outcome)
}

/// Local profile-directory commands; the pin is never displayed.
fn profile_outcome(shell: &ShellArgs, action: ProfileCommand, ctx: &Ctx<'_>) -> Outcome {
    // Local commands never read a secret, but conflicting secret-source
    // selectors are still a typed refusal.
    if let Err(err) = check_secret_conflict(shell) {
        return ("profile", None, Err(err));
    }
    match action {
        ProfileCommand::List => match connect::list() {
            Ok(names) => (
                "profile list",
                None,
                Ok(json!({
                    "profiles": names
                        .iter()
                        .map(|name| json!(output::sanitize(name)))
                        .collect::<Vec<Json>>()
                })),
            ),
            Err(err) => ("profile list", None, Err(err)),
        },
        ProfileCommand::Show { name } => {
            let loaded = match ctx.profile_root {
                Some(root) => connect::load_from(root, &name),
                None => connect::load(&name),
            };
            match loaded {
                Ok(profile) => ("profile show", None, Ok(profile_record(&name, &profile))),
                Err(err) => ("profile show", None, Err(err)),
            }
        }
    }
}

/// Projects one profile to the output schema; the pin is a presence flag.
fn profile_record(name: &str, profile: &Profile) -> Json {
    json!({
        "name": output::sanitize(name),
        "address": output::sanitize(&profile.address),
        "transport": output::sanitize(&profile.transport),
        "username": output::sanitize(&profile.username),
        "database": profile.database.as_deref().map(output::sanitize),
        "pin_present": profile.pin.is_some(),
    })
}

/// Ensures a live session (implicit dial when none); the routing database
/// is validated before any dial or secret read; a failed implicit dial
/// sticks the disconnected sentinel.
fn with_session<'a>(
    shell: &ShellArgs,
    mode: Mode,
    session: &'a mut Option<Session>,
    runtime: &tokio::runtime::Runtime,
    require_database: bool,
    // A fresh shorter lifetime: the returned references borrow only
    // `session`, never the context, and unifying both parameters on `'a`
    // would force every caller's `session` to outlive its `Ctx`.
    ctx: &mut Ctx<'_>,
) -> Result<(&'a Profile, &'a mut Client), ShellError> {
    if session.is_none() {
        let established = establish_profile(shell, mode, None, runtime, require_database, ctx);
        match established {
            Ok((profile, client)) => {
                *session = Some(Session {
                    profile: Some(profile),
                    client: Some(client),
                    dead: false,
                });
            }
            Err(err) => {
                *session = Some(Session {
                    profile: None,
                    client: None,
                    dead: true,
                });
                return Err(err);
            }
        }
    }
    let session = session.as_mut().expect("session established above");
    if session.dead {
        return Err(ShellError::Connection);
    }
    match (session.profile.as_ref(), session.client.as_mut()) {
        (Some(profile), Some(client)) => Ok((profile, client)),
        _ => Err(ShellError::Usage),
    }
}

/// Shared establish path: profile load → route check → transport/pin
/// validation → secret source → secret read → dial (`require_database`
/// enforces the catalogue routing floor).
fn establish_profile(
    shell: &ShellArgs,
    mode: Mode,
    name: Option<&str>,
    runtime: &tokio::runtime::Runtime,
    require_database: bool,
    ctx: &mut Ctx<'_>,
) -> Result<(Profile, Client), ShellError> {
    let profile = load_profile(shell, name, ctx)?;
    if require_database && profile.database.is_none() {
        return Err(ShellError::Usage);
    }
    let connection = connect::prepare(&profile)?;
    let source = secret_source(shell, mode)?;
    // The no-echo prompt opens the injected terminal adapter.
    let secret = terminal::secret::read_secret_with_writer(source, ctx.terminal, ctx.stderr)?;
    let client = runtime.block_on(guarded(async move {
        connection
            .connect(secret)
            .await
            .map_err(connect::request_error)
    }))?;
    Ok((profile, client))
}

/// Resolves the profile name (command argument first, then `--profile`).
fn load_profile(
    shell: &ShellArgs,
    name: Option<&str>,
    ctx: &Ctx<'_>,
) -> Result<Profile, ShellError> {
    let name = name.or(shell.profile.as_deref()).ok_or(ShellError::Usage)?;
    ctx.load_profile(name)
}

/// Resolves the secret source: no clap ArgGroup — multiple explicit
/// selectors are the typed [`ShellError::SecretSourceConflict`] refusal;
/// the hidden prompt is the default ONLY interactively; `--secret-fd` is
/// Unix-only and refused before any secret read or network activity.
fn secret_source(shell: &ShellArgs, mode: Mode) -> Result<SecretSource, ShellError> {
    check_secret_conflict(shell)?;
    #[cfg(windows)]
    if shell.secret_fd.is_some() {
        return Err(ShellError::SecretFdWindows);
    }
    if let Some(fd) = shell.secret_fd {
        let fd = i32::try_from(fd).map_err(|_| ShellError::Usage)?;
        return Ok(SecretSource::Fd(fd));
    }
    if shell.secret_stdin {
        return Ok(SecretSource::Stdin);
    }
    if let Some(name) = shell.secret_env.as_deref() {
        // Design §3.2: the variable name is fixed (the value can be visible
        // to other host processes).
        if name != "SHAMIR_SHELL_PASSWORD" {
            return Err(ShellError::Usage);
        }
        return Ok(SecretSource::Env);
    }
    if shell.secret_prompt {
        return Ok(SecretSource::Prompt);
    }
    // The hidden prompt is the default only in interactive mode.
    if mode == Mode::Interactive {
        Ok(SecretSource::Prompt)
    } else {
        Err(ShellError::InsecureSecret)
    }
}

/// Mutual-exclusion check for the secret-source selectors; also called by
/// the local `profile` commands (which never read a secret).
fn check_secret_conflict(shell: &ShellArgs) -> Result<(), ShellError> {
    let explicit = usize::from(shell.secret_stdin)
        + usize::from(shell.secret_fd.is_some())
        + usize::from(shell.secret_env.is_some())
        + usize::from(shell.secret_prompt);
    if explicit > 1 {
        return Err(ShellError::SecretSourceConflict);
    }
    Ok(())
}

/// Bounds one awaited request by the timeout; Ctrl+C → `Interrupted`; on
/// timeout the connection must be treated as gone.
async fn guarded<T>(future: impl Future<Output = Result<T, ShellError>>) -> Result<T, ShellError> {
    let guarded = async {
        tokio::select! {
            outcome = future => outcome,
            _ = tokio::signal::ctrl_c() => Err(ShellError::Interrupted),
        }
    };
    match tokio::time::timeout(connect::REQUEST_TIMEOUT, guarded).await {
        Ok(outcome) => outcome,
        Err(_) => Err(ShellError::Timeout),
    }
}

/// One verbose informational line on stderr only (never secret material).
fn note(verbose: bool, text: &str) {
    if verbose {
        let mut stderr = io::stderr();
        let _ = writeln!(stderr, "{text}");
    }
}
