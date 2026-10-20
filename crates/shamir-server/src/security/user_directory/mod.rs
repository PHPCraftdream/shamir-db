//! Durable [`FjallUserDirectory`] backed by `fjall` (spec §1.2 + §1.3 + §3.5 + §6.2).
//!
//! Each user record is persisted to a fjall keyspace keyed by username.
//! After every accepted write we call `db.persist(PersistMode::SyncAll)`
//! so the on-disk state always reflects what the server has acknowledged
//! (fsync semantics required by spec §3.5 / §6.2).
//!
//! Roles live alongside the SCRAM-only [`UserRecord`](shamir_connect::server::user_record::UserRecord)
//! inside the persisted blob — `shamir-connect` does not yet model roles in
//! its snapshot type, so we expose them via a separate
//! [`FjallUserDirectory::lookup_roles`] method.
//!
//! ## Atomicity
//!
//! `insert`/`remove` update three keyspaces (username→blob,
//! user_id→username, principal64→username). fjall's `OwnedWriteBatch`
//! (`db.batch()`) commits cross-keyspace ops atomically, so the three
//! indices never disagree.
//!
//! Read-modify-write operations (`update_roles`, `bump_tickets_invalid`)
//! serialise through a single in-process `parking_lot::Mutex` — admin
//! mutations are low-frequency, so the serialisation cost is irrelevant.
//!
//! ## Boot-time normalization (design §6 item 2, task #556)
//!
//! Every `open()` re-derives the `principal64` index from the immutable
//! `user_id` index and re-encodes any record still carrying the legacy
//! `"superuser"` role string into the new `superuser: bool` flag. Both
//! steps are idempotent (a pure re-projection / a `contains` check that is
//! simply false on already-migrated data). The projection step fails
//! `open()` CLOSED on a zero projection (reserved for `OWNER_SYSTEM`) or a
//! collision between two distinct usernames — see
//! [`project_user_ids_to_principal64`].

mod directory;
mod record;

pub use directory::FjallUserDirectory;
#[cfg(test)]
pub(crate) use record::project_user_ids_to_principal64;
#[cfg(test)]
pub(crate) use record::PersistedUser;
pub use record::UserDirectoryState;
