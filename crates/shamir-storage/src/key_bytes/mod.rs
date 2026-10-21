//! Representation-transparent small-buffer key — `KeyBytes`.
//!
//! Implements step 1 of
//! `docs/dev-artifacts/design/record-key-128-migration-plan.md` (task #491): a new
//! representation-transparent small-string-optimized byte-string type,
//! fully tested, **with zero call-site changes anywhere else** in the
//! workspace. `crates/shamir-storage/src/types.rs`'s `pub type
//! RecordKey = Bytes;` alias is left untouched; this type lands
//! alongside it, currently unused by production code.
//!
//! # Design (see plan doc §3)
//!
//! `KeyBytes` is a small-string-optimized key: bytes ≤ `INLINE_CAP`
//! live inline (no heap allocation), longer bytes live in a heap
//! `bytes::Bytes`. The representation is **UNOBSERVABLE** — `Eq`,
//! `Ord`, `Hash`, and `serde` are defined over the byte slice only.
//! This is the single most important correctness constraint (plan doc
//! §5.1 — the #489-class `Value<Key>` Hash/Eq landmine): a key built
//! inline MUST compare, order, and hash identically to the same bytes
//! arriving via a heap `Bytes` (e.g. a key read back from a fjall
//! scan).
//!
//! # Size (deviation from the plan doc, documented)
//!
//! The plan doc specified `INLINE_CAP = 30` with a total size of 32
//! bytes, sized under the assumption that `bytes::Bytes` is 24 bytes.
//! On this target `bytes::Bytes` (1.11) is itself **32 bytes** (it
//! carries an inline-repr niche), so a plain safe tagged enum of
//! `{ inline: {len, [u8; N]}, heap: Bytes }` is 32 bytes only for
//! `N ≤ 23` (verified empirically; `N ≥ 24` jumps to 40 bytes because
//! the inline variant payload then exceeds the 24-byte heap payload).
//!
//! This implementation therefore pins **`INLINE_CAP = 23`**, the
//! largest inline capacity that keeps `size_of::<KeyBytes>() == 32`
//! (the plan doc's hard size gate, mandatory test category 4). This
//! still covers the three hot key shapes the audit targets —
//! `RecordId` (16 B), `WalActiveKey` (21 B), and the 9-byte index
//! prefix — inline. The 25-byte unique-index key and the 41-byte
//! posting key spill to heap, which the plan doc §2 explicitly
//! accepts ("Posting keys (41 B) stay heap unless the cap is raised —
//! acceptable; they are built once per posting write, not per
//! comparison"). Raising the cap to 30 without growing past 32 bytes
//! would require an `unsafe` union/`Bytes`-niche layout; that is left
//! to a separate, measurement-driven follow-up rather than risk UB
//! here.
//!
//! # Serde wire-format invariance (plan doc §5.3)
//!
//! `KeyBytes` serializes as a plain `serde_bytes` byte-blob — exactly
//! the encoding `shamir_wal::wal_entry_v2::serde_bytes_bytes` uses for
//! `Bytes` today (`serde_bytes::Bytes::new(b).serialize(s)` /
//! `serde_bytes::ByteBuf::deserialize`). This makes the on-disk /
//! on-wire encoding of a key independent of whether it is currently
//! represented inline or as a heap `Bytes`, so a later step that flips
//! `type RecordKey = Bytes` to `type RecordKey = KeyBytes` introduces
//! no WAL/disk format change (mandatory test category 3 guards this
//! byte-for-byte against `Bytes` under both `bincode` and `rmp-serde`).
//!
//! # No derived `PartialEq`/`Eq`/`PartialOrd`/`Ord`/`Hash`
//!
//! Every one of these traits is hand-implemented and goes through
//! [`KeyBytes::as_slice`]. A `#[derive(...)]` over `Repr` would
//! compare/hash the `len` byte + tag + padding, leaking the
//! representation and producing exactly the silent-equality-divergence
//! bug this type exists to prevent.

mod core;

pub use core::{KeyBytes, INLINE_CAP};

#[cfg(test)]
mod tests;
