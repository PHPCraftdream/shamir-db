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
//! | Op                 | Canonical input                                                              |
//! |---------------------|------------------------------------------------------------------------------|
//! | drop_db             | `b"drop_db\0<db>"`                                                           |
//! | drop_repo           | `b"drop_repo\0<db_in_use>\0<repo>"`                                          |
//! | drop_table          | `b"drop_table\0<db_in_use>\0<repo>\0<table>"`                                |
//! | drop_index          | `b"drop_index\0<db_in_use>\0<repo>\0<table>\0<index>\0<unique:0|1>"`         |
//! | drop_user           | `b"drop_user\0<username>"`                                                   |
//! | drop_role           | `b"drop_role\0<role>"`                                                       |
//! | grant_role          | `b"grant_role\0<role>\0<user>"`                                              |
//! | revoke_role         | `b"revoke_role\0<role>\0<user>"`                                             |
//! | chmod               | `b"chmod\0<resource>\0<mode>"`                                               |
//! | chown               | `b"chown\0<resource>\0<owner>"`                                              |
//! | chgrp               | `b"chgrp\0<resource>\0<group|null>"`                                         |
//! | create_user         | `b"create_user\0<username>"` (password NEVER included)                       |
//! | create_role         | `b"create_role\0<role>"` (permissions NOT included, mirrors `drop_role`)     |
//! | set_retention       | `b"set_retention\0<db_in_use>\0<repo>\0<table>\0<retention>"`                |
//! | purge_history       | `b"purge_history\0<db_in_use>\0<repo>\0<table>\0<scope>"`                    |
//!
//! `<db_in_use>` is the `db_name` the client passed to
//! `client.execute(db_name, batch)` — server fills it in from the
//! request envelope before validating.
//!
//! `<resource>` is the canonical rendering of a `ResourceRef` produced by
//! [`canonical_resource_ref`] — the same `scheme://path` shape as
//! `shamir_types::access::ResourcePath`'s `Display` impl, but built
//! directly from the wire-level `ResourceRef` so it needs no
//! `server`-feature conversion (client and server must both be able to
//! compute it).
//!
//! `<retention>` / `<scope>` are produced by [`canonical_retention`] /
//! [`canonical_purge_scope`] — stable textual forms of `Retention` /
//! `PurgeScope` (neither type has an existing `Display`/canonical
//! serialization to reuse, so this module defines the one both sides
//! agree on).
//!
//! | create_group        | `b"create_group\0<name>"`                                                    |
//! | drop_group          | `b"drop_group\0<group_ref>"`                                                 |
//! | rename_group        | `b"rename_group\0<group_ref>\0<to>"`                                         |
//! | add_group_member    | `b"add_group_member\0<group_ref>\0<user>"`                                   |
//! | remove_group_member | `b"remove_group_member\0<group_ref>\0<user>"`                                |
//!
//! `<group_ref>` is produced by [`canonical_group_ref`] — a stable
//! `"name:<name>"` / `"id:<id>"` rendering of `GroupRef`'s two variants
//! that can never collide between variants: `"name:"` / `"id:"` prefixes
//! are reserved tags, not part of either variant's raw payload space, so
//! a group literally named `"id:3"` canonicalizes to `"name:id:3"` —
//! distinct from `GroupRef::Id { id: 3 }`'s `"id:3"`.

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

pub fn canonical_grant_role(role: &str, user: &str) -> Vec<u8> {
    join_null(&[b"grant_role", role.as_bytes(), user.as_bytes()])
}

pub fn canonical_revoke_role(role: &str, user: &str) -> Vec<u8> {
    join_null(&[b"revoke_role", role.as_bytes(), user.as_bytes()])
}

pub fn canonical_create_user(username: &str) -> Vec<u8> {
    // Password is NEVER part of the canonical input — the tag confirms
    // "you meant to create this account", not the credential.
    join_null(&[b"create_user", username.as_bytes()])
}

pub fn canonical_create_role(role: &str) -> Vec<u8> {
    // Permissions are not part of the canonical input, mirroring
    // `drop_role`'s precedent of identifying the op by name only.
    join_null(&[b"create_role", role.as_bytes()])
}

/// Render a [`crate::admin::ResourceRef`] into the stable `scheme://path`
/// string used by every `canonical_*` chmod/chown/chgrp helper below.
/// Mirrors `shamir_types::access::ResourcePath`'s `Display` shape, but is
/// built directly from the wire-level `ResourceRef` (no `server`-feature
/// conversion) so client and server compute byte-identical strings.
pub fn canonical_resource_ref(r: &crate::admin::ResourceRef) -> String {
    use crate::admin::ResourceRef;
    match r {
        ResourceRef::Database { database } => format!("db://{database}"),
        ResourceRef::Store { store: [db, s] } => format!("db://{db}/{s}"),
        ResourceRef::Table { table: [db, s, t] } => format!("db://{db}/{s}/{t}"),
        ResourceRef::Function { function } => format!("fn://{function}"),
        ResourceRef::FunctionFolder { function_folder } => {
            format!("fn://{}/", function_folder.join("/"))
        }
        ResourceRef::FunctionNamespace { .. } => "fn://".to_string(),
    }
}

pub fn canonical_chmod(resource: &crate::admin::ResourceRef, mode: u16) -> Vec<u8> {
    join_null(&[
        b"chmod",
        canonical_resource_ref(resource).as_bytes(),
        mode.to_string().as_bytes(),
    ])
}

pub fn canonical_chown(resource: &crate::admin::ResourceRef, owner: u64) -> Vec<u8> {
    join_null(&[
        b"chown",
        canonical_resource_ref(resource).as_bytes(),
        owner.to_string().as_bytes(),
    ])
}

/// `group: None` (clear the group) canonicalizes to the literal sentinel
/// `"null"` — chosen because it can never collide with a valid decimal
/// `u64` group id.
pub fn canonical_chgrp(resource: &crate::admin::ResourceRef, group: Option<u64>) -> Vec<u8> {
    let group_str = match group {
        Some(g) => g.to_string(),
        None => "null".to_string(),
    };
    join_null(&[
        b"chgrp",
        canonical_resource_ref(resource).as_bytes(),
        group_str.as_bytes(),
    ])
}

/// Render a [`crate::admin::Retention`] into the stable textual form used
/// by [`canonical_set_retention`]. Neither this module nor `Retention`
/// itself has an existing `Display`/canonical serialization, so this is
/// the one both client and server agree on: each of the three orthogonal
/// optional knobs rendered as its decimal value or the sentinel `"none"`,
/// comma-joined in field-declaration order.
pub fn canonical_retention(r: &crate::admin::Retention) -> String {
    let age = r
        .max_age_secs
        .map(|v| v.to_string())
        .unwrap_or_else(|| "none".to_string());
    let max = r
        .max_count
        .map(|v| v.to_string())
        .unwrap_or_else(|| "none".to_string());
    let min = r
        .min_count
        .map(|v| v.to_string())
        .unwrap_or_else(|| "none".to_string());
    format!("{age},{max},{min}")
}

pub fn canonical_set_retention(
    db_in_use: &str,
    repo: &str,
    table: &str,
    retention: &crate::admin::Retention,
) -> Vec<u8> {
    join_null(&[
        b"set_retention",
        db_in_use.as_bytes(),
        repo.as_bytes(),
        table.as_bytes(),
        canonical_retention(retention).as_bytes(),
    ])
}

/// Render a [`crate::admin::PurgeScope`] into the stable textual form used
/// by [`canonical_purge_history`]. `PurgeScope` has no existing
/// `Display`/canonical serialization, so this defines the agreed form:
/// `"older_than:<timestamp>"` or `"older_than_age:<age_secs>"`.
pub fn canonical_purge_scope(scope: &crate::admin::PurgeScope) -> String {
    use crate::admin::PurgeScope;
    match scope {
        PurgeScope::OlderThan { timestamp } => format!("older_than:{timestamp}"),
        PurgeScope::OlderThanAge { age_secs } => format!("older_than_age:{age_secs}"),
    }
}

pub fn canonical_purge_history(
    db_in_use: &str,
    repo: &str,
    table: &str,
    scope: &crate::admin::PurgeScope,
) -> Vec<u8> {
    join_null(&[
        b"purge_history",
        db_in_use.as_bytes(),
        repo.as_bytes(),
        table.as_bytes(),
        canonical_purge_scope(scope).as_bytes(),
    ])
}

/// Render a [`crate::admin::GroupRef`] into the stable `name:<name>` /
/// `id:<id>` string used by every `canonical_*` group-op helper below.
/// Exhaustive match, no wildcard — a future `GroupRef` variant that isn't
/// handled here fails to compile instead of silently falling through.
///
/// The `"name:"` / `"id:"` prefixes are reserved tags outside either
/// variant's raw payload space, so the two variants can never collide:
/// a group literally named `"id:3"` renders as `"name:id:3"`, not
/// `"id:3"` (which only `GroupRef::Id { id: 3 }` produces).
pub fn canonical_group_ref(r: &crate::admin::GroupRef) -> String {
    use crate::admin::GroupRef;
    match r {
        GroupRef::Name { name } => format!("name:{name}"),
        GroupRef::Id { id } => format!("id:{id}"),
    }
}

pub fn canonical_create_group(name: &str) -> Vec<u8> {
    join_null(&[b"create_group", name.as_bytes()])
}

pub fn canonical_drop_group(group: &crate::admin::GroupRef) -> Vec<u8> {
    join_null(&[b"drop_group", canonical_group_ref(group).as_bytes()])
}

pub fn canonical_rename_group(group: &crate::admin::GroupRef, to: &str) -> Vec<u8> {
    join_null(&[
        b"rename_group",
        canonical_group_ref(group).as_bytes(),
        to.as_bytes(),
    ])
}

pub fn canonical_add_group_member(group: &crate::admin::GroupRef, user: u64) -> Vec<u8> {
    join_null(&[
        b"add_group_member",
        canonical_group_ref(group).as_bytes(),
        user.to_string().as_bytes(),
    ])
}

pub fn canonical_remove_group_member(group: &crate::admin::GroupRef, user: u64) -> Vec<u8> {
    join_null(&[
        b"remove_group_member",
        canonical_group_ref(group).as_bytes(),
        user.to_string().as_bytes(),
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

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(TABLE[(b >> 4) as usize] as char);
        s.push(TABLE[(b & 0x0f) as usize] as char);
    }
    s
}

pub(crate) fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
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
