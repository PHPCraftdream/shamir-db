use shamir_connect::common::crypto::try_random_array;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::server::user_record::UserRecord;
use zeroize::Zeroizing;

/// Derive a SCRAM `UserRecord` (Argon2id) from a plaintext password under
/// the given KDF params. CPU-bound work is delegated to `spawn_blocking`.
/// Shared between `create_scram_user` (the wire `CreateScramUser` path)
/// and the `UserAdminPort::create_user` impl (task #559) so the Argon2id
/// code is not duplicated a third time. Returns `Err(message)` on a
/// derivation or join failure (callers surface it as a `code: "query"`
/// error / `PortError` respectively).
pub async fn derive_scram_record(password: String, kdf: KdfParams) -> Result<UserRecord, String> {
    // Move password into a zeroizing buffer right away. `Zeroizing`
    // wipes on Drop, so we don't need an explicit `.zeroize()` call —
    // both the success and error paths drop `pw_buf` before returning.
    let pw_buf: Zeroizing<Vec<u8>> = Zeroizing::new(password.into_bytes());
    let salt: [u8; 16] = try_random_array().map_err(|e| e.to_string())?;

    // Argon2id is CPU-heavy — delegate to spawn_blocking so the runtime
    // worker is free to make progress on other tasks during derivation.
    let derive_result = tokio::task::spawn_blocking(move || {
        DerivedKeys::derive(&pw_buf, &salt, &kdf).map(|d| (d, salt))
    })
    .await;

    let (derived, salt) = match derive_result {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => return Err(format!("argon2id: {e}")),
        Err(join_err) => return Err(format!("argon2id task failed: {join_err}")),
    };

    let mut server_key_z: Zeroizing<[u8; 32]> = Zeroizing::new([0u8; 32]);
    server_key_z.copy_from_slice(&derived.server_key[..]);
    Ok(UserRecord {
        salt,
        stored_key: derived.stored_key,
        server_key: server_key_z,
        kdf_params: kdf,
        tickets_invalid_before_ns: 0,
    })
}
