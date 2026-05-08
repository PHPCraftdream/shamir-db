//! Client-side of the connection protocol.
//!
//! Implementation order (per spec):
//! 1. Argon2id derivation: `salted_password → client_key, server_key, stored_key`
//! 2. `client_proof` builder: `client_key XOR HMAC(stored_key, auth_message)`
//! 3. Server response verification: SCRAM mutual auth + Ed25519 identity + pin
//! 4. Resumption ticket consumption (encrypt/decrypt is server-only; client stores opaque blob)
//! 5. `changePassword` two-step state machine
//!
//! Currently stub: foundation modules in [`crate::common`] are required first.

#![allow(missing_docs)]
