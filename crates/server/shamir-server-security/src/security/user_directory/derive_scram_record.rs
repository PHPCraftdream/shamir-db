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
/// password under the given KDF params. Admission and its three-second permit
/// budget are fixed before submitting to `spawn_blocking`: the budget is a
/// wall-clock deadline plus the monotonic instant of the same reading, so the
/// job keeps paying for its blocking-pool queue time instead of restarting the
/// budget at the semaphore. An admitted job retains its RAII admission ticket
/// (and that budget) through salt generation, KDF, record assembly, and
/// password-buffer zeroization.
///
/// The plaintext is consumed into a `Zeroizing` buffer synchronously, before
/// the future exists, so a never-polled and dropped future retains no
/// unzeroized password bytes.
///
/// cancel-safe: yes — dropping this future after submission detaches the
/// blocking job, whose admission ticket is retained until the job and its
/// zeroizing password buffer finish; if dropped before polling, no job or
/// admission is created and the synchronously-created password buffer is
/// zeroized when the future is dropped.
pub fn derive_scram_record_gated(
    password: String,
    kdf: KdfParams,
    gate: Arc<ScramDerivationGate>,
) -> impl Future<Output = Result<UserRecord, ScramDerivationError>> + Send {
    let pw_buf: Zeroizing<Vec<u8>> = Zeroizing::new(password.into_bytes());

    async move {
        // Admission and deadline are fixed before the blocking pool can queue
        // this job. A full gate returns Busy without submitting any work.
        let ticket = gate.admit()?;
        let budget = ticket.budget;
        let join_result = tokio::task::spawn_blocking(move || {
            // Locals drop in reverse declaration order: the zeroizing password
            // buffer is cleared before the admission ticket releases capacity.
            let _ticket = ticket;
            let pw_buf = pw_buf;
            let salt =
                try_random_array().map_err(|e| ScramDerivationError::Failed(e.to_string()))?;
            let derived = gate
                .run(budget, || DerivedKeys::derive(&pw_buf, &salt, &kdf))?
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
