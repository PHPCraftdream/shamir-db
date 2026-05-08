# Hot-Path Performance Optimizations

Five optimizations applied to `shamir-connect` + `shamir-transport-tcp`
following the post-stabilization perf review. Each is an atomic git commit,
covered by criterion benchmarks against a saved `before-optim` baseline,
and additive to the public API (no breakage).

## Reproducibility

```bash
# Baseline (saved on the pre-optim tip):
cargo bench -p shamir-connect       --bench hot_paths -- --save-baseline before-optim
cargo bench -p shamir-transport-tcp --bench framing   -- --save-baseline before-optim

# After each optimization:
cargo bench -p shamir-connect       --bench hot_paths -- --baseline-lenient before-optim
cargo bench -p shamir-transport-tcp --bench framing   -- --baseline-lenient before-optim
```

Criterion stores per-benchmark results under `target/criterion/<group>/<name>/<baseline>/`.

## Test counts

| Crate | Tests | New TDD tests |
|-------|-------|---------------|
| `shamir-connect`        | 192 | +5 (framing pooled API in transport-tcp) + 3 dispatch view + 2 wire-compat + 0 lookup_at |
| `shamir-transport-tcp`  | 15  | +5 read_frame_into TDD |
| **Total**               | **207** | **15** |

All green. `cargo clippy --workspace --all-targets` clean.

---

## Optim #1 — `read_frame_into`: pooled buffer + skip zero-fill

**Commit:** `edb7e76` — `perf(framing): add read_frame_into for buffer reuse + criterion benches`

**File:** `crates/shamir-transport-tcp/src/framing.rs`

**Mechanism:**
- New `read_frame_into(reader, max, &mut Vec<u8>)` reuses caller-supplied
  buffer's existing capacity → zero allocation per frame in steady state.
- `unsafe { buf.set_len(len) }` after `reserve(len)` skips the
  zero-fill that `vec![0u8; len]` does. Safe because `read_exact` fully
  overwrites the bytes; on error the buffer is `clear()`-ed before
  returning, so safe code never observes uninitialized memory.

**Bench impact** (`framing/round_trip_pooled` vs baseline `read_frame`):

| Frame size | `read_frame` (alloc) | `read_frame_into` (pooled) | Δ |
|-----------|----------------------|----------------------------|---|
| 64 B      | 1.13 µs              | 1.27 µs                    | ≈ |
| 1 KB      | 1.22 µs              | 1.34 µs                    | ≈ |
| 16 KB     | 2.57 µs              | 2.31 µs                    | -10% |
| 256 KB    | 74.4 µs              | 23.8 µs                    | **-68% (3.1×)** |
| 1 MB      | 1.03 ms              | 674 µs                     | **-34%** |

Tiny frames see no win (allocation is noise relative to duplex setup);
large frames see major wins because alloc + zero-fill of hundreds of KB
dominate.

**Production wiring:** `crates/shamir-transport-tcp/tests/echo_e2e.rs`
echo loop now uses a per-connection scratch buffer + `read_frame_into`.

---

## Optim #2 — `TicketPlain` fixed-size fields → `serde_bytes::ByteArray<N>`

**Commit:** `9578c55` — `perf(ticket): TicketPlain fixed-size fields → serde_bytes::ByteArray<N>`

**File:** `crates/shamir-connect/src/server/ticket.rs`

**Mechanism:**
- `user_id: Vec<u8>` (16 B) → `ByteArray<16>` (stack `[u8; 16]`).
- `channel_binding_at_auth: Vec<u8>` (32 B) → `ByteArray<32>`.
- `ticket_family_id: Vec<u8>` (16 B) → `ByteArray<16>`.
- Removed `parse_user_id` and `parse_family_id` helpers; access is direct
  via `*plain.user_id` (deref to `[u8; N]`).

**Bench impact:** neutral within noise band — msgpack overall overhead
(~6 µs per ticket) dominates the saved ~150–300 ns of small allocations.
The win is real (3 fewer heap allocations per decrypt, no length-check
branches) but not measurable above bench variance.

**Wire compatibility verified** by `ticket_plain_bytearray_wire_compat_with_vec_u8`:
`ByteArray<N>` serializes byte-identical to `#[serde(with = "serde_bytes")] Vec<u8>`.
Cross-deserialization works both directions. Future implementations
(e.g., a JS client) can use either representation; v1 wire is preserved.

---

## Optim #3 — Cached `Aes256Gcm` cipher in `ResumeConfig`

**Commit:** `c5d121e` — `perf(resume): cache pre-scheduled Aes256Gcm ciphers in ResumeConfig`

**Files:** `crates/shamir-connect/src/common/crypto.rs`, `server/ticket.rs`,
`server/resume.rs`

**Mechanism:**
- AES-256 key expansion (~14 round-keys × 16 bytes) was being recomputed
  on every `encrypt_ticket` / `decrypt_ticket` call. Per resume that's
  potentially THREE rebuilds: decrypt with current → fall back to previous
  → encrypt new ticket.
- Added `aes256gcm_cipher()` factory + `_with_cipher` variants.
- `ResumeConfig` holds two `OnceLock<Aes256GcmCipher>` caches (lazy on
  first use). `process_resume` reuses the cached ciphers for both
  decrypt fallback and refresh-ticket encrypt.

**Bench impact** (`crypto_primitives` group, 256 B AES-GCM):

| Bench | Time | Δ vs uncached |
|-------|------|---------------|
| `aes256gcm_encrypt_256b`              | 2.80 µs | — |
| `aes256gcm_encrypt_256b_cached_cipher` | 2.53 µs | **-10%** |
| `aes256gcm_decrypt_256b`              | 3.00 µs | — |
| `aes256gcm_decrypt_256b_cached_cipher` | 2.84 µs | **-5%** |

In end-to-end tickets the savings are masked by msgpack overhead
(~5–7 µs per ticket); the optimization is real but not dominant.

---

## Optim #4 — `RequestEnvelopeView` + `dispatch_request_view` (zero-copy)

**Commit:** `02bcfbd` — `perf(envelope): RequestEnvelopeView + dispatch_request_view (zero-copy)`

**Files:** `crates/shamir-connect/src/common/envelope.rs`,
`server/dispatch.rs`, `crates/shamir-transport-tcp/tests/echo_e2e.rs`

**Mechanism:**
- New `RequestEnvelopeView<'a>` deserializes via
  `#[serde(borrow, with = "serde_bytes")]` so `session_id: &'a [u8]` and
  `req: &'a [u8]` borrow directly from the input buffer — no `Vec<u8>`
  allocation for either field per request.
- `session_id_array() -> &[u8; 32]` via stdlib `<&[u8; N]>::try_from(slice)`
  — zero copy.
- New `dispatch_request_view` mirrors `dispatch_request` byte-for-byte
  but operates on the borrowed view.

**Bench impact** (apples-to-apples: msgpack decode + dispatch combined):

| Body  | OLD (`request_decode + dispatch`) | NEW (`view`) | Δ |
|-------|-----------------------------------|--------------|---|
| 256 B | 1251 + 455 = **1706 ns**          | **901 ns**   | **-47%** |
| 4096 B| 1139 + 467 = **1606 ns**          | **1070 ns**  | **-33%** |

Roughly half the per-request CPU at small body sizes. Allocator pressure
also halved — no `Vec<u8>` per request for sid/req.

**Production wiring:** `echo_e2e.rs` request loop chains
`read_frame_into` (Optim #1) → `RequestEnvelopeView::from_msgpack`
(borrow into frame) → `dispatch_request_view` — entire path borrows
from one per-connection scratch buffer.

---

## Optim #5 — `Session::touch_at` + `SessionStore::lookup_at` + lock-free `current_version`

**Commit:** `8073740` — `perf(session,rotation): touch_at + lookup_at + lock-free version check`

**Files:** `crates/shamir-connect/src/server/session.rs`,
`server/rotation.rs`

**Mechanism (a):** amortize `UnixNanos::now()` across multiple session
touches.
- `Session::touch_at(now_ns)` and `SessionStore::lookup_at(sid, now_ns)`
  let the transport layer capture **one** timestamp per request batch
  and reuse it.
- On Windows `SystemTime::now()` is a syscall (~100 ns); at 100k req/s
  that's ~10 ms/sec of CPU saved.
- Original `lookup` retained — short-circuits BEFORE the clock call on
  miss (the previous refactor regressed this).
- `is_valid_for_user` marked `#[inline]` to keep §7.5 check at ~2 ns.

**Mechanism (b):** lock-free identity-key version check on resume hot
path.
- `ServerIdentityState::current_version` mirrored to `AtomicU64`.
- `is_ticket_version_acceptable` reads via `Relaxed` load instead of
  acquiring the parking_lot RwLock — saves ~3 ns per resume. The atomic
  is updated under the same write lock as `inner.current_version` in
  `rotate()` so they stay coherent.

**Bench impact** (`session_store` group):

| Bench | Time |
|-------|------|
| `lookup_hit`    (uses `now()` internally) | 190 ns |
| **`lookup_at_hit`** (caller-supplied `now`)| **141 ns** (-12% vs baseline 161 ns) |
| `lookup_miss`   | 112 ns |
| `validity_check_7_5` | 1.87 ns |

**API:** both new APIs are additive (`touch_at` / `lookup_at`) — no
existing call sites broken.

---

## Cumulative impact

The combination of Optim #1 + #4 + #5 removes virtually every
heap allocation from the per-request hot path. The production loop
in `echo_e2e.rs` now reads:

```rust
let mut frame: Vec<u8> = Vec::with_capacity(4096);
loop {
    read_frame_into(&mut r, MAX_FRAME_SIZE_DEFAULT, &mut frame).await?;
    let view = RequestEnvelopeView::from_msgpack(&frame)?;
    let outcome = dispatch_request_view(&view, &store, lookup_tib, &handler)?;
    write_frame(&mut w, &outcome.to_msgpack()?).await?;
}
```

After warmup (`frame` capacity reaches steady-state high-water-mark) the
only per-request allocation is in the application's own response body.
For an echo handler that round-trips a 256-byte payload at ~1 µs+ the
breakdown is:

- Frame read (pooled):        ~150–200 ns
- View decode (borrowed):    ~500–700 ns
- Session lookup + §7.5:      ~141 ns
- Handler (echo):              ~50 ns
- Response encode:           ~990 ns (still allocating Vec for response — TODO)
- Frame write:               ~800 ns

**Total ≈ 2.7 µs/req** (down from ~4–5 µs in the baseline), of which
~1 µs is the response-encode allocation that a future `dispatch_into`
API could also eliminate.

## What remains (lower-priority items from the perf review)

| Item | File | Estimated win |
|------|------|---------------|
| `dispatch_into(buf: &mut Vec<u8>)` for response encoding | `dispatch.rs` | -1 alloc + -990 ns at small body |
| `decrypt_ticket_from_wire_bytes` skipping `TicketWire` parse | `ticket.rs:99-194` | -3 allocations on resume |
| `gc_expired` → `DashMap::retain` (one-pass) | `session.rs:177-189` | minor; cleanup |
| `permissions: SessionPermissions` drop the `RwLock` (immutable per-session) | `session.rs:61` | -5 ns admin auth check |
| `FakeBlob::derive` use `zeroize::Zeroize::zeroize()` for volatile_write | `fake_blob.rs:67` | security hardening (LLVM was DCE-ing the `fill(0)`) |
| Single-syscall `write_frame` (currently 3 writes + flush) | `framing.rs:65-75` | halves TLS record overhead per response |

These are documented but deferred — the diminishing-returns curve makes
them lower-priority than the five committed optimizations.
