//! Representation-transparent small-buffer key — `KeyBytes`.
//!
//! Implements step 1 of
//! `docs/design/record-key-128-migration-plan.md` (task #491): a new
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

use bytes::Bytes;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;

/// Inline capacity — the largest byte length stored without a heap
/// allocation. See the [Size](self#size-deviation-from-the-plan-doc-documented)
/// section of the module docs for why this is 23 rather than the plan
/// doc's nominal 30.
pub const INLINE_CAP: usize = 23;

/// Small-string-optimized key: inline for `len <= INLINE_CAP`, heap
/// `Bytes` beyond. Representation is UNOBSERVABLE — see the module docs.
pub struct KeyBytes(Repr);

/// Private representation. NEVER `#[derive]` `PartialEq`/`Eq`/`Hash`/
/// `Ord`/`PartialOrd`/`Serialize`/`Deserialize` on this enum — every
/// observable trait is hand-implemented on `KeyBytes` over the byte
/// slice.
enum Repr {
    /// `len` is the live byte count; only `buf[..len]` is meaningful.
    Inline { len: u8, buf: [u8; INLINE_CAP] },
    /// Heap-backed; the canonical fallback for `len > INLINE_CAP` and
    /// for the test-only forced-heap path.
    Heap(Bytes),
}

impl KeyBytes {
    /// The workhorse constructor — inline when the input fits in
    /// [`INLINE_CAP`], otherwise heap-copies into a [`Bytes`].
    /// Alloc-free for `bytes.len() <= INLINE_CAP`.
    #[inline]
    pub fn from_slice(bytes: &[u8]) -> Self {
        if bytes.len() <= INLINE_CAP {
            let mut buf = [0u8; INLINE_CAP];
            buf[..bytes.len()].copy_from_slice(bytes);
            KeyBytes(Repr::Inline {
                len: bytes.len() as u8,
                buf,
            })
        } else {
            KeyBytes(Repr::Heap(Bytes::copy_from_slice(bytes)))
        }
    }

    /// Borrowed byte view. This is the single source of truth for every
    /// observable trait (`Eq`/`Ord`/`Hash`/`Serialize`/`Debug`); all of
    /// them go through here so representation is unobservable.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        match &self.0 {
            Repr::Inline { len, buf } => &buf[..*len as usize],
            Repr::Heap(b) => b.as_ref(),
        }
    }

    /// `true` if this key is currently stored inline (no heap
    /// allocation). Gated to test builds — representation is otherwise
    /// an unobservable implementation detail.
    #[cfg(test)]
    pub(crate) fn is_inline(&self) -> bool {
        matches!(self.0, Repr::Inline { .. })
    }

    /// Test-only constructor that FORCES the heap variant even for a
    /// short (`<= INLINE_CAP`) input. Used by the inline-vs-heap
    /// Eq/Hash consistency test (mandatory test category 2) to prove
    /// that representation does not leak through any observable trait.
    #[cfg(test)]
    pub(crate) fn force_heap_for_test(bytes: &[u8]) -> Self {
        KeyBytes(Repr::Heap(Bytes::copy_from_slice(bytes)))
    }
}

// ---- Conversions in -------------------------------------------------------

impl From<Bytes> for KeyBytes {
    /// Inline-copy when the input fits in [`INLINE_CAP`] (cheap), else
    /// take the `Bytes` verbatim into the heap variant (refcount bump,
    /// zero-copy).
    #[inline]
    fn from(b: Bytes) -> Self {
        if b.len() <= INLINE_CAP {
            // Short input — copy inline so future clones / compares stay
            // in registers; the source `Bytes` refcount then drops.
            Self::from_slice(b.as_ref())
        } else {
            KeyBytes(Repr::Heap(b))
        }
    }
}

impl From<Vec<u8>> for KeyBytes {
    #[inline]
    fn from(v: Vec<u8>) -> Self {
        if v.len() <= INLINE_CAP {
            Self::from_slice(&v)
        } else {
            KeyBytes(Repr::Heap(Bytes::from(v)))
        }
    }
}

impl From<&'static [u8]> for KeyBytes {
    #[inline]
    fn from(b: &'static [u8]) -> Self {
        Self::from_slice(b)
    }
}

impl From<KeyBytes> for Bytes {
    /// Heap variant: zero-copy move out of the `Bytes`. Inline variant:
    /// one heap-allocating copy — this is a cold boundary conversion,
    /// not a hot path (plan doc §3).
    #[inline]
    fn from(k: KeyBytes) -> Self {
        match k.0 {
            Repr::Heap(b) => b,
            Repr::Inline { len, buf } => Bytes::copy_from_slice(&buf[..len as usize]),
        }
    }
}

// ---- Slice-view traits ----------------------------------------------------

impl Deref for KeyBytes {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl AsRef<[u8]> for KeyBytes {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Borrow<[u8]> for KeyBytes {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}

// ---- Clone / Debug --------------------------------------------------------

impl Clone for KeyBytes {
    /// Inline = memcpy of the 32-byte slot; heap = refcount bump.
    #[inline]
    fn clone(&self) -> Self {
        match &self.0 {
            Repr::Inline { len, buf } => KeyBytes(Repr::Inline {
                len: *len,
                buf: *buf,
            }),
            Repr::Heap(b) => KeyBytes(Repr::Heap(b.clone())),
        }
    }
}

impl fmt::Debug for KeyBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Delegate to the byte slice's Debug so the representation
        // (inline vs heap) is invisible in diagnostic output too.
        fmt::Debug::fmt(self.as_slice(), f)
    }
}

// ---- Eq / Ord / Hash — hand-written, all via as_slice() -------------------
//
// These are the load-bearing correctness impls (plan doc §5.1). Do NOT
// replace with `#[derive(...)]`: a derived impl would compare/hash `len`
// + tag + padding and leak the representation.

impl PartialEq for KeyBytes {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for KeyBytes {}

impl PartialOrd for KeyBytes {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for KeyBytes {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl Hash for KeyBytes {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

// ---- Cross-type PartialEq conveniences (slice-delegating) -----------------

impl PartialEq<[u8]> for KeyBytes {
    #[inline]
    fn eq(&self, other: &[u8]) -> bool {
        self.as_slice() == other
    }
}

impl PartialEq<Bytes> for KeyBytes {
    #[inline]
    fn eq(&self, other: &Bytes) -> bool {
        self.as_slice() == other.as_ref()
    }
}

// ---- Serde — byte-blob identical to `serde_bytes` encoding of `Bytes` -----
//
// Mirrors `shamir_wal::wal_entry_v2::serde_bytes_bytes` exactly:
//   serialize:   serde_bytes::Bytes::new(b.as_ref()).serialize(s)
//   deserialize: serde_bytes::ByteBuf::deserialize(d) -> Bytes::from(vec)
// This is the on-disk / on-wire invariance guard for a later alias flip
// (plan doc §5.3) — never replace with `#[derive(Serialize, Deserialize)]`,
// which would add a variant tag and break WAL replay.

impl Serialize for KeyBytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serde_bytes::Bytes::new(self.as_slice()).serialize(s)
    }
}

impl<'de> Deserialize<'de> for KeyBytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let bb = serde_bytes::ByteBuf::deserialize(d)?;
        Ok(Self::from_slice(bb.as_ref()))
    }
}

#[cfg(test)]
mod tests;
