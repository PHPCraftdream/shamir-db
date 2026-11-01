use super::{create::CliIo, secret_output, TokenAction};
use crate::config::Config;
/// Dispatch a token action with injected output sinks.
pub async fn run_with_io(c: &Config, a: TokenAction, io: &mut CliIo<'_>) -> anyhow::Result<()> {
    match a {
        TokenAction::Create(x) => super::create::run_create(c, x, io).await,
        TokenAction::List(x) => super::list::run_list(c, x, io).await,
        TokenAction::Revoke { id } => {
            super::revoke::run_revoke(c, super::revoke::Action::One(id), io).await
        }
        TokenAction::RevokeUser { user } => {
            super::revoke::run_revoke(c, super::revoke::Action::User(user), io).await
        }
        TokenAction::RevokeAll { yes } => {
            anyhow::ensure!(yes, "revoke-all requires --yes");
            super::revoke::run_revoke(c, super::revoke::Action::All, io).await
        }
    }
}
/// Run an offline token command using process stdout/stderr.
pub async fn run(c: &Config, a: TokenAction) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let mut io = CliIo {
        stdout: &mut stdout,
        stderr: &mut stderr,
        is_terminal: secret_output::stdout_is_terminal(),
    };
    run_with_io(c, a, &mut io).await
}
