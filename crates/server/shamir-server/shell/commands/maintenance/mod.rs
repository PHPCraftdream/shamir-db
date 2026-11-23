//! Maintenance commands (design `docs/dev-artifacts/design/admin-shell.md`
//! §4.7–§4.10, §9 PR5): the online-copy trio, the format-stamp and
//! schema-migration views, the WAL/durability diagnostics and the
//! explained-only offline manuals. One file per topic; this file is
//! declarations and re-exports ONLY.
mod backup;
mod command;
mod diag;
mod explain;
mod versions;

pub(in crate::shell) use command::MaintenanceCommand;
