//! Terminal (TTY/console) secret entry for the admin shell.
//!
//! Design: `docs/dev-artifacts/design/admin-shell.md` §3.2 (secret sources),
//! §6 (no echo, restore on every exit path, no history). Every secret source
//! funnels through the bounded line reader in `secret.rs` (4096-byte cap,
//! `BSTDIN:1-60` pattern); the platform files (`unix.rs`, `windows.rs`)
//! implement only the no-echo hidden-line session behind the `Terminal`
//! seam, so a scripted fake can drive the same disable → read → restore
//! orchestration.

pub(crate) mod secret;
#[cfg(unix)]
pub(crate) mod unix;
#[cfg(windows)]
pub(crate) mod windows;

pub(crate) use secret::SecretSource;
