//! Admin-mutation inherent methods on
//! [`FjallUserDirectory`](super::FjallUserDirectory): user creation
//! (`insert_with_scope`, `insert_disabled_import`), removal (`remove`),
//! and privilege changes (`set_superuser`, `set_replicator`). Split out
//! of `directory.rs` to stay under the 1000-line layout cap; declared as
//! a child module of `directory` so it can see the type's private
//! fields, mirroring the `table_manager/operations` split pattern.

use std::sync::atomic::Ordering;

use fjall::PersistMode;
use shamir_connect::common::error::{Error, Result};

use shamir_connect::server::user_record::UserRecord;

use super::super::record::PersistedUser;
use super::FjallUserDirectory;
use shamir_types::access::principal64;

impl FjallUserDirectory {
    /// Permanently delete a user account: all three keyspaces, atomically.
    ///
    /// Does NOT evict live sessions itself — `FjallUserDirectory` has no
    /// handle to a `SessionStore`. Per the existing pattern in
    /// `crates/net/shamir-connect/src/server/admin.rs` (`snapshot_by_user` then
    /// kill, already used by the role-update/credential-change paths), the
    /// CALLER is responsible for snapshotting and killing live sessions for
    /// this `user_id` after a successful `remove()` — this is out of this
    /// task's scope (the wire-level `DropUser` handler wiring is task #559's
    /// job).
    ///
    /// Refuses to remove the last remaining superuser account
    /// (last-superuser guard) — returns a typed error, deletes nothing.
    ///
    /// Returns `Ok(false)` for an already-absent username (idempotent no-op),
    /// `Ok(true)` if the account was removed.
    pub fn remove(&self, username: &str) -> Result<bool> {
        let _guard = self.write_lock.lock();

        let blob = match self.read_blob(username)? {
            Some(b) => b,
            None => return Ok(false), // already absent — idempotent no-op
        };
        let user: PersistedUser = rmp_serde::from_slice(&blob)
            .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;
        let user_id = user
            .user_id_array()
            .ok_or_else(|| Error::Encoding("corrupt user_id in persisted record".to_string()))?;

        if user.superuser && self.superuser_count.load(Ordering::Relaxed) <= 1 {
            return Err(Error::InvalidInput(
                "cannot remove the last remaining superuser account",
            ));
        }

        let projected = principal64(user_id);
        let pkey = projected.to_be_bytes();

        let mut batch = self.db.batch();
        batch.remove(&self.users, username.as_bytes());
        batch.remove(&self.user_id_index, &user_id[..]);
        batch.remove(&self.principal64_index, &pkey[..]);
        batch
            .commit()
            .map_err(|e| Error::Encoding(format!("fjall: batch commit: {e}")))?;

        // Spec §3.5 / §6.2: fsync before returning.
        self.db
            .persist(PersistMode::SyncAll)
            .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

        // Evict the tickets_cache entry too — the cache is the AUTHORITATIVE
        // source `tickets_invalid_before_ns_by_user_id` reads from. A stale
        // cache entry surviving past `remove()` would make a deleted
        // account's user_id still resolve to a (stale) tib value instead of
        // the fail-open 0 — reproducing exactly the fail-open bug the
        // `UserStateLookup` fix closes, just via a different path. This is
        // NOT optional.
        let _ = self.tickets_cache.remove_sync(&user_id);

        if user.superuser {
            self.superuser_count.fetch_sub(1, Ordering::Relaxed);
        }

        Ok(true)
    }

    /// Grant or revoke superuser status (task #557). Idempotent (no-op if
    /// the flag is already at the requested value, returns `Ok(false)` with
    /// no write). On an actual change: bumps `tickets_invalid_before_ns`
    /// (spec §12.6 — a privilege change must invalidate existing sessions,
    /// same rule as `update_roles`), persists with `SyncAll`, updates the
    /// `tickets_cache`, and adjusts `superuser_count` atomically in the
    /// SAME critical section as the blob mutation.
    ///
    /// Refuses to revoke the LAST remaining superuser (uses the O(1)
    /// `superuser_count` warmed at `open()` and maintained by `remove()` +
    /// this method) — mirrors `remove()`'s last-superuser guard so the
    /// system can never lock itself out of admin.
    ///
    /// This is deliberately a bespoke method (not routed through
    /// `read_modify_write`) because it needs to adjust `superuser_count`
    /// in the SAME critical section as the blob mutation — `read_modify_write`'s
    /// closure has no way to touch the counter.
    pub fn set_superuser(&self, username: &str, on: bool, now_ns: u64) -> Result<bool> {
        let _guard = self.write_lock.lock();

        let blob = match self.read_blob(username)? {
            Some(b) => b,
            None => return Err(Error::UserNotFound),
        };
        let mut user: PersistedUser = rmp_serde::from_slice(&blob)
            .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;

        if user.superuser == on {
            return Ok(false); // already at the requested state — no-op
        }
        // Last-superuser guard: refuse to revoke if this is the only one.
        if user.superuser && !on && self.superuser_count.load(Ordering::Relaxed) <= 1 {
            return Err(Error::InvalidInput(
                "cannot revoke superuser status from the last remaining superuser account",
            ));
        }

        user.superuser = on;
        // Spec §12.6: privilege change must invalidate existing sessions.
        if now_ns > user.tickets_invalid_before_ns {
            user.tickets_invalid_before_ns = now_ns;
        }
        let new_bytes = rmp_serde::to_vec_named(&user)
            .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;
        self.users
            .insert(username.as_bytes(), new_bytes.as_slice())
            .map_err(|e| Error::Encoding(format!("fjall: insert: {e}")))?;
        // Spec §3.5 / §6.2: fsync before returning.
        self.db
            .persist(PersistMode::SyncAll)
            .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

        if let Some(id) = user.user_id_array() {
            self.update_cache(&id, user.tickets_invalid_before_ns);
        }
        if on {
            self.superuser_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.superuser_count.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(true)
    }

    /// Grant or revoke replication-API access (task #621). Mirrors
    /// [`Self::set_superuser`] almost literally — same idempotent no-op
    /// shape, same §12.6 privilege-change tickets-invalidation bump, same
    /// cache update — but deliberately WITHOUT a last-remaining guard or a
    /// counter: unlike superuser (which must always have at least one
    /// holder so the system can't lock itself out of admin), zero
    /// replicators is a perfectly normal state — nothing is gated on "how
    /// many are left".
    pub fn set_replicator(&self, username: &str, on: bool, now_ns: u64) -> Result<bool> {
        let _guard = self.write_lock.lock();

        let blob = match self.read_blob(username)? {
            Some(b) => b,
            None => return Err(Error::UserNotFound),
        };
        let mut user: PersistedUser = rmp_serde::from_slice(&blob)
            .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;

        if user.replicator == on {
            return Ok(false); // already at the requested state — no-op
        }

        user.replicator = on;
        // Spec §12.6-style: privilege change must invalidate existing sessions.
        if now_ns > user.tickets_invalid_before_ns {
            user.tickets_invalid_before_ns = now_ns;
        }
        let new_bytes = rmp_serde::to_vec_named(&user)
            .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;
        self.users
            .insert(username.as_bytes(), new_bytes.as_slice())
            .map_err(|e| Error::Encoding(format!("fjall: insert: {e}")))?;
        self.db
            .persist(PersistMode::SyncAll)
            .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

        if let Some(id) = user.user_id_array() {
            self.update_cache(&id, user.tickets_invalid_before_ns);
        }
        Ok(true)
    }

    // ------------------------------------------------------------------
    // Task #559: directory methods backing the PrincipalResolver /
    // UserAdminPort seam. These live on `FjallUserDirectory` itself (not
    // just the adapter) so they share the directory's `write_lock`
    // serialisation and fjall atomicity guarantees — the adapter stays a
    // thin passthrough.
    // ------------------------------------------------------------------

    /// Insert a fresh user carrying a `database` scope, parallel to the
    /// `insert` trait method (which cannot carry
    /// `database` — `shamir-connect`'s `UserRecord` is SCRAM-only). Used
    /// by the `UserAdminPort::create_user` impl so the scope is set
    /// atomically with creation. Roles start empty here; the port attaches
    /// them via `update_roles` (which enforces the `"superuser"` string
    /// reservation).
    pub fn insert_with_scope(
        &self,
        username: String,
        record: UserRecord,
        database: Option<String>,
    ) -> Result<[u8; 16]> {
        let _guard = self.write_lock.lock();

        let exists = self
            .users
            .contains_key(username.as_bytes())
            .map_err(|e| Error::Encoding(format!("fjall: contains_key: {e}")))?;
        if exists {
            return Err(Error::InvalidInput("username exists"));
        }

        let user_id = self.mint_unique_user_id()?;
        let projected = principal64(user_id);
        let pkey = projected.to_be_bytes();

        let persisted = PersistedUser::from_record(user_id, &record, Vec::new(), false, database);
        let bytes = rmp_serde::to_vec_named(&persisted)
            .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;

        let mut batch = self.db.batch();
        batch.insert(&self.users, username.as_bytes(), bytes.as_slice());
        batch.insert(&self.user_id_index, &user_id[..], username.as_bytes());
        batch.insert(&self.principal64_index, &pkey[..], username.as_bytes());
        batch
            .commit()
            .map_err(|e| Error::Encoding(format!("fjall: batch commit: {e}")))?;

        self.db
            .persist(PersistMode::SyncAll)
            .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

        self.update_cache(&user_id, persisted.tickets_invalid_before_ns);

        Ok(user_id)
    }

    /// Insert a fresh DISABLED identity for the release-1 offline logical
    /// import. Mirrors [`Self::insert_with_scope`]'s id-minting and three-
    /// keyspace batch plumbing exactly, but the caller supplies NO
    /// credentials: `salt`/`stored_key`/`server_key` are filled from the OS
    /// CSPRNG (via [`PersistedUser::from_imported_disabled`] — nothing
    /// constant or guessable is stored, so no password can ever match) and
    /// the record is marked `disabled = true`, so every authentication path
    /// (which collapses on `lookup_by_name` returning `None`) refuses the
    /// account with the same generic failure as an unknown user. Returns the
    /// minted user id like `insert_with_scope` does.
    pub fn insert_disabled_import(
        &self,
        username: String,
        database: Option<String>,
    ) -> Result<[u8; 16]> {
        let _guard = self.write_lock.lock();

        let exists = self
            .users
            .contains_key(username.as_bytes())
            .map_err(|e| Error::Encoding(format!("fjall: contains_key: {e}")))?;
        if exists {
            return Err(Error::InvalidInput("username exists"));
        }

        let user_id = self.mint_unique_user_id()?;
        let projected = principal64(user_id);
        let pkey = projected.to_be_bytes();

        let persisted = PersistedUser::from_imported_disabled(user_id, database)?;
        let bytes = rmp_serde::to_vec_named(&persisted)
            .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;

        let mut batch = self.db.batch();
        batch.insert(&self.users, username.as_bytes(), bytes.as_slice());
        batch.insert(&self.user_id_index, &user_id[..], username.as_bytes());
        batch.insert(&self.principal64_index, &pkey[..], username.as_bytes());
        batch
            .commit()
            .map_err(|e| Error::Encoding(format!("fjall: batch commit: {e}")))?;

        self.db
            .persist(PersistMode::SyncAll)
            .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

        self.update_cache(&user_id, persisted.tickets_invalid_before_ns);

        Ok(user_id)
    }
}
