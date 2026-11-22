use shamir_connect::common::crypto::try_random_array;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::server::user_record::UserRecord;
use std::future::Future;
use std::sync::Arc;
use zeroize::Zeroizing;

use super::scram_gate::{ScramDerivationError, ScramDerivationGate};

/// Shared `UserRecord` assembly from an already-derived key set. Kept
/// private: both wrappers (ungated and gated) build the record the same
/// way, and the gate permit never encloses this — only the KDF call.
fn user_record(derived: DerivedKeys, salt: [u8; 16], kdf: KdfParams) -> UserRecord {
    let mut server_key_z: Zeroizing<[u8; 32]> = Zeroizing::new([0u8; 32]);
    server_key_z.copy_from_slice(&derived.server_key[..]);
    UserRecord {
        salt,
        stored_key: derived.stored_key,
        server_key: server_key_z,
        kdf_params: kdf,
        tickets_invalid_before_ns: 0,
    }
}

/// Derive a SCRAM `UserRecord` (Argon2id) from a plaintext password under
/// the given KDF params, *without* taking an `Argon2Semaphore` permit.
/// Intentionally ungated and used only for bootstrap credential rotation,
/// including a successful first token login (which may happen at a point
/// where the gate already exists); all other callers must use
/// `derive_scram_record_gated`. CPU-bound work is delegated to
/// `spawn_blocking`; derivation or join failures are returned as
/// `Err(message)` for the caller to surface.
pub async fn derive_scram_record(password: String, kdf: KdfParams) -> Result<UserRecord, String> {
    let pw_buf: Zeroizing<Vec<u8>> = Zeroizing::new(password.into_bytes());
    let salt: [u8; 16] = try_random_array().map_err(|e| e.to_string())?;

    let derive_result = tokio::task::spawn_blocking(move || {
        DerivedKeys::derive(&pw_buf, &salt, &kdf)
            .map(|keys| user_record(keys, salt, kdf))
            .map_err(|e| format!("argon2id: {e}"))
    })
    .await;

    match derive_result {
        Ok(record) => record,
        Err(join_err) => Err(format!("argon2id task failed: {join_err}")),
    }
}

/// Gated derivation of a SCRAM `UserRecord` (Argon2id) from a plaintext
/// password under the given KDF params, holding an `Argon2Semaphore`
/// permit around the Argon2id KDF only (via `ScramDerivationGate::run`),
/// inside a `spawn_blocking` task.
///
/// The plaintext is consumed into a `Zeroizing` buffer synchronously,
/// before the future exists, so a never-polled and dropped future
/// retains no unzeroized password bytes.
///
/// cancel-safe: yes — if the caller drops the future before it
/// resolves, the detached blocking task may still finish, but it holds
/// and reclaims its own permit and persists no result anywhere; the
/// synchronous zeroization above has already happened before any polling.
pub fn derive_scram_record_gated(
    password: String,
    kdf: KdfParams,
    gate: Arc<ScramDerivationGate>,
) -> impl Future<Output = Result<UserRecord, ScramDerivationError>> + Send {
    let pw_buf: Zeroizing<Vec<u8>> = Zeroizing::new(password.into_bytes());

    async move {
        let join_result = tokio::task::spawn_blocking(move || {
            let salt =
                try_random_array().map_err(|e| ScramDerivationError::Failed(e.to_string()))?;
            let derived = gate
                .run(|| DerivedKeys::derive(&pw_buf, &salt, &kdf))?
                .map_err(|e| ScramDerivationError::Failed(format!("argon2id: {e}")))?;
            Ok(user_record(derived, salt, kdf))
        })
        .await;

        match join_result {
            Ok(result) => result,
            Err(join_err) => Err(ScramDerivationError::Failed(format!(
                "argon2id task failed: {join_err}"
            ))),
        }
    }
}
