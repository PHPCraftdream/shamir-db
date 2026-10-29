//! End-to-end backup → restore round-trip.
//!
//! Strategy:
//!   1. Boot ServerLauncher#1 against a fresh data_dir.
//!   2. Real client logs in, writes a row.
//!   3. Cleanly shutdown.
//!   4. `backup::backup(data_dir, snapshot_dir)` — copies every file.
//!   5. **Wipe the original data_dir** (simulating disk loss).
//!   6. **Restore**: copy the backup back into a fresh data_dir.
//!   7. Boot ServerLauncher#2 with the restored data_dir.
//!   8. Same client (same admin password) logs in, reads the row, asserts
//!      it matches what was written in boot #1.
//!
//! This proves the WHOLE backup pipeline is sound — not just the file-copy
//! mechanics (which `backup::tests` already cover) but every durable
//! piece the server depends on:
//!   * server_meta.redb     (Ed25519 identity, audit chain key, ticket key)
//!   * users.redb           (admin SCRAM credentials)
//!   * counters.redb        (replay counters)
//!   * shamir_db_meta.redb  (database / repo metadata)
//!   * shamir_db_default_main.redb (the data we wrote)
//!   * wire_tables.mpack    (registry of tables created over the wire)
//!   * cert.pem / key.pem   (TLS material — without it the second boot
//!   * cert.pem / key.pem   (TLS material — without it the second boot
//!     would generate a NEW Ed25519 + new server pub-key,
//!     breaking client TOFU pin sanity)

mod backup_roundtrip;
mod common;
mod restore_cli;
mod restore_failure;

mod unclean_restore;
