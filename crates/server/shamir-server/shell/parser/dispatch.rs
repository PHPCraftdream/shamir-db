//! Per-command dispatch (design §§4.1, §5): the §3.3 token-subset policy,
//! session gating, and the outcome of each parsed command (connect, ping,
//! local profile commands, catalogue commands). No argv handling here.

use std::io::Write;

use serde_json::{json, Value as Json};

use super::grammar::{
    command_id, AuthKind, Command, DbCommand, FunctionCommand, IndexCommand, Mode, Outcome,
    ProfileCommand, RepoCommand, ShellArgs, TableCommand,
};
use super::session::{
    check_secret_conflict, establish_profile, guarded, with_session, Ctx, Session,
};
use super::state_args::{DataCommand, ReplicationCommand, VersionCommand};
use crate::shell::commands::{Catalogue, StateCommand};
use crate::shell::connect;
use crate::shell::connect::{Profile, SessionClient};
use crate::shell::output::{self, ShellError};

/// Closed §3.3 policy. Connect is lifecycle, not a remote data command.
fn allowed_with_token(command: &Command) -> bool {
    match command {
        Command::Connect { .. }
        | Command::Ping
        | Command::Db {
            action: DbCommand::List,
        }
        | Command::Repo {
            action: RepoCommand::List,
        }
        | Command::Table {
            action:
                TableCommand::List { .. } | TableCommand::Describe { .. } | TableCommand::Schema { .. },
        }
        | Command::Index {
            action: IndexCommand::List { .. },
        }
        | Command::Function {
            action: FunctionCommand::List { .. },
        }
        // §3.3: only the two narrow data views are in the read-only token
        // subset; `status`, `data get`, `replication subscriptions` and
        // `version show` are refused before any connect or request.
        | Command::Data {
            action: DataCommand::Count { .. } | DataCommand::Head { .. },
        } => true,
        Command::Profile { .. }
        | Command::Repo {
            action: RepoCommand::Describe { .. },
        }
        | Command::Status
        | Command::Version { .. }
        | Command::Replication { .. }
        | Command::Data {
            action: DataCommand::Get { .. },
        } => false,
    }
}

/// Dispatches one parsed command; batch callers pass a fresh `None` session.
pub(super) fn dispatch(
    shell: &ShellArgs,
    mode: Mode,
    command: Command,
    session: &mut Option<Session>,
    runtime: &tokio::runtime::Runtime,
    ctx: &mut Ctx<'_>,
) -> Outcome {
    let id = command_id(&command);
    let auth = session.as_ref().map_or(shell.auth, |state| state.auth);
    let token = session
        .as_ref()
        .and_then(|state| state.live.as_ref())
        .map_or(auth == AuthKind::Token, |(_, client)| client.is_token());
    if token && !allowed_with_token(&command) {
        return (id, None, Err(ShellError::TokenRequestDenied));
    }
    if let (false, None, Err(err)) = (
        matches!(command, Command::Connect { .. }),
        session.as_ref(),
        auth.check(),
    ) {
        return (id, None, Err(err));
    }
    let which = match command {
        Command::Connect { name, auth } => {
            let auth = auth.unwrap_or(shell.auth);
            // Drop the old connection before any profile, prompt or dial.
            *session = Some(Session { auth, live: None });
            let outcome = establish_profile(
                shell,
                mode,
                (auth, name.as_deref()),
                runtime,
                false,
                true,
                ctx,
            )
            .map(|(profile, client)| {
                *session = Some(Session {
                    auth,
                    live: Some((profile, client)),
                });
                note(shell.verbose, "connected");
                json!({ "connected": true })
            });
            return (id, None, outcome);
        }
        Command::Ping => {
            let outcome = with_session(shell, mode, session, runtime, false, ctx)
                .and_then(|(_, client)| runtime.block_on(guarded(ping(client))));
            return (id, Some("Ping"), outcome.map(|()| json!({ "ok": true })));
        }
        Command::Profile { action } => return profile_outcome(shell, action, ctx),
        Command::Db {
            action: DbCommand::List,
        } => Catalogue::Databases,
        Command::Repo { action } => match action {
            RepoCommand::List => Catalogue::Repos,
            RepoCommand::Describe { repo } => Catalogue::DescribeRepo { repo },
        },
        Command::Table { action } => match action {
            TableCommand::List { repo } => Catalogue::Tables { repo },
            TableCommand::Describe { repo, table } => Catalogue::Describe { repo, table },
            TableCommand::Schema { repo, table } => Catalogue::Schema { repo, table },
        },
        Command::Index {
            action: IndexCommand::List { repo, table },
        } => Catalogue::Indexes { repo, table },
        Command::Function {
            action: FunctionCommand::List { folder },
        } => Catalogue::Functions { folder },
        Command::Status => {
            return state_outcome(shell, mode, session, runtime, ctx, id, StateCommand::Status)
        }
        Command::Version {
            action: VersionCommand::Show,
        } => {
            return state_outcome(
                shell,
                mode,
                session,
                runtime,
                ctx,
                id,
                StateCommand::VersionShow,
            )
        }
        Command::Replication {
            action: ReplicationCommand::Subscriptions,
        } => {
            return state_outcome(
                shell,
                mode,
                session,
                runtime,
                ctx,
                id,
                StateCommand::ReplicationSubscriptions,
            )
        }
        Command::Data { action } => {
            return state_outcome(
                shell,
                mode,
                session,
                runtime,
                ctx,
                id,
                match action {
                    DataCommand::Count {
                        database,
                        repo,
                        table,
                    } => StateCommand::Count {
                        db: database,
                        repo,
                        table,
                    },
                    DataCommand::Head {
                        database,
                        repo,
                        table,
                        rows,
                    } => StateCommand::Head {
                        db: database,
                        repo,
                        table,
                        rows,
                    },
                    DataCommand::Get {
                        database,
                        repo,
                        table,
                        key,
                    } => StateCommand::Get {
                        db: database,
                        repo,
                        table,
                        key,
                    },
                },
            )
        }
    };
    catalogue(shell, mode, session, runtime, ctx, which)
}

/// State and narrow-data commands (design §§4.2, §4.4), delegated to the
/// state worker (`shell/commands/state.rs`). The §3.3 token gate has already
/// run above; the worker's `validate` (the Base58 KEY check) runs HERE,
/// before any dial or secret read. Only `replication subscriptions` carries
/// the profile routing-database floor; `data *` name their database
/// explicitly and `status`/`version show` are server-wide.
fn state_outcome(
    shell: &ShellArgs,
    mode: Mode,
    session: &mut Option<Session>,
    runtime: &tokio::runtime::Runtime,
    ctx: &mut Ctx<'_>,
    id: &'static str,
    action: StateCommand,
) -> Outcome {
    if let Err(err) = action.validate() {
        return (id, None, Err(err));
    }
    let routing_db_required = action.requires_routing_db();
    let outcome = with_session(shell, mode, session, runtime, routing_db_required, ctx).and_then(
        |(profile, client)| {
            let routing_db = profile.database.as_deref();
            runtime.block_on(guarded(action.execute(client, routing_db)))
        },
    );
    (id, Some(action.op()), outcome)
}

/// cancel-safe: NO — the request may already be sent; the caller drops the session.
async fn ping(client: &SessionClient) -> Result<(), ShellError> {
    client.ping().await.map_err(connect::request_error)
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
            let loaded = ctx.load_profile(&name);
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
        "username": profile.username.as_deref().map(output::sanitize),
        "database": profile.database.as_deref().map(output::sanitize),
        "pin_present": profile.pin.is_some(),
    })
}

/// One verbose informational line on stderr only (never secret material).
fn note(verbose: bool, text: &str) {
    if verbose {
        let mut stderr = std::io::stderr();
        let _ = writeln!(stderr, "{text}");
    }
}
