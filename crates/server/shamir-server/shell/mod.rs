//! The administrative shell package root (design
//! `docs/dev-artifacts/design/admin-shell.md` §2): module declarations and
//! the single public re-export only. All wiring lives in the declared child
//! modules; the early pre-config dispatch in `src/main.rs` reaches the shell
//! exclusively through [`run`].

pub(super) mod commands;
pub(super) mod connect;
pub(super) mod output;
pub(super) mod parser;
pub(super) mod terminal;

pub(super) use parser::run;
