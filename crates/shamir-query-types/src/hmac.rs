//! Canonical HMAC input bytes for destructive admin operations.
//!
//! Server and client both call into this module — they must agree
//! byte-for-byte on what gets HMAC'd. Wire-format-stable: changing
//! a layout here is a breaking protocol change.
//!
//! # Why HMAC at all
//!
//! ShamirDB's transport is already TLS 1.3 + SCRAM-Argon2id;
//! anyone holding a valid `session_id` (the bearer token) can act
//! as the session by construction. The HMAC on `drop_*` operations
//! is therefore NOT an authentication gate — it's a "did you mean
//! it" guard. The client cannot produce the tag by accident: they
//! must explicitly construct the canonical input and run HMAC.
//! Matching tag = confirmation of intent.
//!
//! # Key derivation
//!
//! `key = SHA256("shamir-db hmac key v1\0" || session_id)`
//!
//! Domain-separated so the session_id isn't reused raw as a key.
//! Both sides derive locally; nothing extra over the wire.
//!
//! # Per-op canonical input
//!
//! Null-byte-separated bytes:
//!
//! | Op           | Canonical input                                                              |
//! |--------------|------------------------------------------------------------------------------|
//! | drop_db      | `b"drop_db\0<db>"`                                                           |
//! | drop_repo    | `b"drop_repo\0<db_in_use>\0<repo>"`                                          |
//! | drop_table   | `b"drop_table\0<db_in_use>\0<repo>\0<table>"`                                |
//! | drop_index   | `b"drop_index\0<db_in_use>\0<repo>\0<table>\0<index>\0<unique:0|1>"`         |
//! | drop_user    | `b"drop_user\0<username>"`                                                   |
//! | drop_role    | `b"drop_role\0<role>"`                                                       |
//!
//! `<db_in_use>` is the `db_name` the client passed to
//! `client.execute(db_name, batch)` — server fills it in from the
//! request envelope before validating.

/// 32-byte HMAC key derived from the session bearer token.
pub fn derive_session_hmac_key(session_id: &[u8; 32]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"shamir-db hmac key v1\0");
    h.update(session_id);
    let out = h.finalize();
    let mut k = [0u8; 32];
    k.copy_from_slice(&out);
    k
}

fn join_null(parts: &[&[u8]]) -> Vec<u8> {
    let total: usize = parts.iter().map(|p| p.len()).sum::<usize>() + parts.len().saturating_sub(1);
    let mut out = Vec::with_capacity(total);
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push(0u8);
        }
        out.extend_from_slice(p);
    }
    out
}

pub fn canonical_drop_db(db: &str) -> Vec<u8> {
    join_null(&[b"drop_db", db.as_bytes()])
}

pub fn canonical_drop_repo(db_in_use: &str, repo: &str) -> Vec<u8> {
    join_null(&[b"drop_repo", db_in_use.as_bytes(), repo.as_bytes()])
}

pub fn canonical_drop_table(db_in_use: &str, repo: &str, table: &str) -> Vec<u8> {
    join_null(&[
        b"drop_table",
        db_in_use.as_bytes(),
        repo.as_bytes(),
        table.as_bytes(),
    ])
}

pub fn canonical_drop_index(
    db_in_use: &str,
    repo: &str,
    table: &str,
    index: &str,
    unique: bool,
) -> Vec<u8> {
    let unique_byte: &[u8] = if unique { b"1" } else { b"0" };
    join_null(&[
        b"drop_index",
        db_in_use.as_bytes(),
        repo.as_bytes(),
        table.as_bytes(),
        index.as_bytes(),
        unique_byte,
    ])
}

pub fn canonical_drop_user(username: &str) -> Vec<u8> {
    join_null(&[b"drop_user", username.as_bytes()])
}

pub fn canonical_drop_role(role: &str) -> Vec<u8> {
    join_null(&[b"drop_role", role.as_bytes()])
}

pub fn canonical_start_migration(
    db_in_use: &str,
    src_repo: &str,
    table: &str,
    dst_repo: &str,
    dst_engine: &str,
) -> Vec<u8> {
    join_null(&[
        b"start_migration",
        db_in_use.as_bytes(),
        src_repo.as_bytes(),
        table.as_bytes(),
        dst_repo.as_bytes(),
        dst_engine.as_bytes(),
    ])
}

pub fn canonical_commit_migration(db_in_use: &str, migration_id: &str) -> Vec<u8> {
    join_null(&[
        b"commit_migration",
        db_in_use.as_bytes(),
        migration_id.as_bytes(),
    ])
}

pub fn canonical_rollback_migration(db_in_use: &str, migration_id: &str) -> Vec<u8> {
    join_null(&[
        b"rollback_migration",
        db_in_use.as_bytes(),
        migration_id.as_bytes(),
    ])
}

/// Compute a hex-encoded HMAC-SHA256 tag.
pub fn compute_tag_hex(key: &[u8; 32], canonical: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
    mac.update(canonical);
    let bytes = mac.finalize().into_bytes();
    hex_encode(&bytes)
}

/// Constant-time check of a candidate hex tag against the expected
/// canonical bytes for this op. Returns `true` iff the tag is a
/// valid hex string of correct length AND matches the recomputed
/// HMAC bit-for-bit.
pub fn verify_tag_hex(key: &[u8; 32], canonical: &[u8], candidate_hex: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
    mac.update(canonical);
    let Ok(bytes) = hex_decode(candidate_hex) else {
        return false;
    };
    mac.verify_slice(&bytes).is_ok()
}

// ---- minimal hex codec (no extra deps) ----

fn hex_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(TABLE[(b >> 4) as usize] as char);
        s.push(TABLE[(b & 0x0f) as usize] as char);
    }
    s
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_nibble(c: u8) -> Result<u8, ()> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_deterministic_and_domain_separated() {
        let sid = [7u8; 32];
        let k1 = derive_session_hmac_key(&sid);
        let k2 = derive_session_hmac_key(&sid);
        assert_eq!(k1, k2);
        // Different session → different key.
        let mut other = sid;
        other[0] ^= 0xFF;
        assert_ne!(derive_session_hmac_key(&other), k1);
        // Domain-separated: raw session_id != derived key.
        assert_ne!(k1, sid);
    }

    #[test]
    fn canonical_inputs_are_null_separated() {
        assert_eq!(canonical_drop_db("mydb"), b"drop_db\0mydb");
        assert_eq!(
            canonical_drop_repo("mydb", "cold"),
            b"drop_repo\0mydb\0cold"
        );
        assert_eq!(
            canonical_drop_table("mydb", "main", "users"),
            b"drop_table\0mydb\0main\0users"
        );
        assert_eq!(
            canonical_drop_index("mydb", "main", "users", "by_email", false),
            b"drop_index\0mydb\0main\0users\0by_email\x00"
        );
        assert_eq!(
            canonical_drop_index("mydb", "main", "users", "by_email", true),
            b"drop_index\0mydb\0main\0users\0by_email\x01"
        );
        assert_eq!(canonical_drop_user("bob"), b"drop_user\0bob");
        assert_eq!(canonical_drop_role("admin"), b"drop_role\0admin");

        assert_eq!(
            canonical_start_migration("mydb", "main", "users", "cold", "redb"),
            b"start_migration\0mydb\0main\0users\0cold\0redb"
        );
        assert_eq!(
            canonical_commit_migration("mydb", "mig-001"),
            b"commit_migration\0mydb\0mig-001"
        );
        assert_eq!(
            canonical_rollback_migration("mydb", "mig-001"),
            b"rollback_migration\0mydb\0mig-001"
        );
    }

    #[test]
    fn compute_then_verify_roundtrip() {
        let key = derive_session_hmac_key(&[1u8; 32]);
        let canonical = canonical_drop_table("db", "main", "users");
        let tag = compute_tag_hex(&key, &canonical);
        assert!(verify_tag_hex(&key, &canonical, &tag));
    }

    #[test]
    fn verify_rejects_wrong_input() {
        let key = derive_session_hmac_key(&[1u8; 32]);
        let canonical = canonical_drop_table("db", "main", "users");
        let tag = compute_tag_hex(&key, &canonical);
        // Same key but different op-bytes — must NOT verify.
        let wrong = canonical_drop_table("db", "main", "OTHER");
        assert!(!verify_tag_hex(&key, &wrong, &tag));
    }

    #[test]
    fn verify_rejects_wrong_key() {
        let key_a = derive_session_hmac_key(&[1u8; 32]);
        let key_b = derive_session_hmac_key(&[2u8; 32]);
        let canonical = canonical_drop_table("db", "main", "users");
        let tag = compute_tag_hex(&key_a, &canonical);
        assert!(!verify_tag_hex(&key_b, &canonical, &tag));
    }

    #[test]
    fn verify_rejects_malformed_hex() {
        let key = derive_session_hmac_key(&[1u8; 32]);
        let canonical = canonical_drop_db("x");
        assert!(!verify_tag_hex(&key, &canonical, ""));
        assert!(!verify_tag_hex(&key, &canonical, "not-hex-at-all"));
        assert!(!verify_tag_hex(&key, &canonical, "deadbee")); // odd length
    }

    #[test]
    fn hex_codec_roundtrip() {
        let bytes = [0u8, 1, 15, 16, 255, 0xab];
        let enc = hex_encode(&bytes);
        assert_eq!(enc, "00010f10ffab");
        assert_eq!(hex_decode(&enc).unwrap(), bytes);
        assert_eq!(hex_decode("DEADBEEF").unwrap(), [0xde, 0xad, 0xbe, 0xef]);
    }
}
