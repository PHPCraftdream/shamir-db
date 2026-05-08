//! Server-side of the connection protocol.
//!
//! Implementation order (per spec):
//! 1. SCRAM verify with constant-time real/fake branching (HKDF anti-enum)
//! 2. `server_signature` and `identity_sig` always-compute (no timing oracle)
//! 3. Session struct + per-request validity check (§7.5)
//! 4. Pre-Argon2id `binding_mode` policy enforcement
//! 5. Resumption ticket: AES-256-GCM encrypt/decrypt + per-family counter CAS
//! 6. Bootstrap CAS + `superuser_ever_existed` invariant
//! 7. `changePassword` server flow: pending challenge state machine
//! 8. Identity rotation: broadcast + `rotation_in_progress` orphan recovery
//!
//! Currently stub: foundation modules in [`crate::common`] are required first.

#![allow(missing_docs)]
