//! The shell parser package (design `docs/dev-artifacts/design/admin-shell.md`
//! §§4.1, §4.3, §5): module declarations and re-exports only. The grammar
//! lives in [`grammar`], the `data`/`version`/`replication` grammar tails in
//! [`state_args`], the bounded interactive line reader in [`input`],
//! session establishment and the secret-source policy in [`session`],
//! per-command dispatch in [`dispatch`], and the entry points / run modes in
//! [`run`].

mod dispatch;
mod grammar;
mod input;
pub(crate) mod run;
mod session;
mod state_args;

pub use run::run;
