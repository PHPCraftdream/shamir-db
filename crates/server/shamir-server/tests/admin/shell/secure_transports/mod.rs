//! Secure-transport acceptance against the real listeners (R5 PR2 and the
//! R5-2b review fixes).
//!
//! Declarations only. `capture` owns the child-subprocess scripted REPL
//! (real captured stdout/stderr) and the shared run assertions; `listeners`
//! carries the moved WSS-SCRAM and token-catalogue suites; the remaining
//! files cover one review item each; `server_perimeter` is reserved for the
//! separate 6d step.

mod capture;
mod idle;
mod listeners;
mod misclassified;
mod profile_probes;
mod server_perimeter;
