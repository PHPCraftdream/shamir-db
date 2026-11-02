//! Task #559 §8: one-time boot audit (read-only, WARN-log only, no
//! auto-apply). Removable after one release.

use crate::user_directory::FjallUserDirectory;

/// Diff shamir-db's Store B (`users`/`roles` tables) against the directory
/// and `tracing::warn!` one line per divergence found. READ-ONLY: never
/// mutates either store.
///
/// This is a standalone function so it can be unit-tested independently of
/// the full boot sequence (see `boot_audit_tests.rs`). Called once at boot
/// from `ServerLauncher::launch` after both `ShamirDb` and
/// `FjallUserDirectory` are constructed.
///
/// What it checks (per design doc §6 item 3):
///   - Usernames present in Store B but not in the directory (phantom users
///     that never had a live login).
///   - Usernames present in the directory but not in Store B (fine — just
///     informational, no action needed).
///   - Role sets that diverge between the two stores.
///   - Phantom `superuser` grants in Store B that were never live.
///
/// Each WARN line carries an explicit "these had no effect and were not
/// applied" trailer.
pub async fn audit_store_b_vs_directory(shamir: &shamir_db::ShamirDb, dir: &FjallUserDirectory) {
    // Read Store B users. Errors here are non-fatal — an empty/missing
    // Store B just means there's nothing to audit.
    let store_b_users: Vec<shamir_types::types::value::QueryValue> =
        match shamir.system_store().load_users().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "boot audit: failed to read Store B users table, skipping audit"
                );
                return;
            }
        };

    // Read directory principals.
    let dir_entries = match dir.list_all() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "boot audit: failed to list directory users, skipping audit"
            );
            return;
        }
    };

    // Build lookup maps.
    use std::collections::BTreeMap;
    let mut dir_map: BTreeMap<String, (Vec<String>, bool)> = BTreeMap::new();
    for (_p64, state) in &dir_entries {
        let mut roles = state.roles.clone();
        // The directory's `superuser` flag is separate from role strings;
        // for comparison, if the flag is set, represent it as a synthetic
        // "superuser" string so Store B's role-string-based grants compare
        // apples-to-apples.
        if state.superuser && !roles.iter().any(|r| r == "superuser") {
            roles.push("superuser".to_string());
        }
        roles.sort();
        dir_map.insert(state.username.clone(), (roles, state.superuser));
    }

    let mut store_b_by_name: BTreeMap<String, (Vec<String>, bool)> = BTreeMap::new();
    for rec in &store_b_users {
        let name = match rec.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let roles: Vec<String> = rec
            .get("roles")
            .and_then(|v| v.as_array())
            .map(|arr| {
                let mut r: Vec<String> = arr
                    .iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect();
                r.sort();
                r
            })
            .unwrap_or_default();
        let has_superuser = roles.iter().any(|r| r == "superuser");
        store_b_by_name.insert(name, (roles, has_superuser));
    }

    // Diffs.
    let dir_names: std::collections::BTreeSet<&str> = dir_map.keys().map(|s| s.as_str()).collect();
    let store_b_names: std::collections::BTreeSet<&str> =
        store_b_by_name.keys().map(|s| s.as_str()).collect();

    // Store B users not in directory (phantom — these never had live login).
    for name in store_b_names.difference(&dir_names) {
        let (roles, su) = &store_b_by_name[*name];
        tracing::warn!(
            user = %name,
            roles = ?roles,
            had_superuser = %su,
            "boot audit: user exists in Store B (shamir-db's retired users table) \
             but NOT in the real directory — these had no effect and were not applied"
        );
    }

    // Users in both stores: check role divergence.
    for name in store_b_names.intersection(&dir_names) {
        let (store_b_roles, store_b_su) = &store_b_by_name[*name];
        let (dir_roles, dir_su) = &dir_map[*name];
        if store_b_roles != dir_roles {
            tracing::warn!(
                user = %name,
                store_b_roles = ?store_b_roles,
                directory_roles = ?dir_roles,
                "boot audit: role sets diverge between Store B and the directory \
                 — these had no effect and were not applied"
            );
        }
        // Phantom superuser: Store B has it but directory doesn't.
        if *store_b_su && !*dir_su {
            tracing::warn!(
                user = %name,
                "boot audit: phantom superuser grant in Store B — the directory \
                 does NOT have this user as superuser; these had no effect and \
                 were not applied"
            );
        }
    }

    // Directory users not in Store B: informational only (the directory is
    // canonical). Log at debug to avoid noise.
    for name in dir_names.difference(&store_b_names) {
        tracing::debug!(
            user = %name,
            "boot audit: user exists in directory but not Store B (expected — \
             directory is canonical)"
        );
    }
}
