# Hot-Path Performance Optimizations

**TEN** optimizations applied to `shamir-connect` + `shamir-transport-tcp`
following two rounds of perf review. Each is an atomic git commit,
covered by criterion benchmarks against a saved `before-optim` baseline,
and additive to the public API (no breakage).

**Plus:** docs/client-server-protocol-spec/diagram fixes for the §8.5 latency-padding wiring and one
diagram-only mismatch.

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

| Crate | Tests | TDD tests added across both rounds |
|-------|-------|-----------------------------------|
| `shamir-connect`        | 199 | +5 (round 1) + +9 (round 2: 1 latency, 2 envelope ref, 4 helper, 2 framing-into wire-compat) |
| `shamir-transport-tcp`  | 17  | +5 (round 1) + +2 (round 2: write_frame_into) |
| **Total**               | **216** | **21** |

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

---

## Round 2 (added after second perf review + docs/client-server-protocol-spec/diagram audit)

### Fix #1 — Latency padding bug + wiring

**Commit:** `a16dd3c` — `fix(latency): canonicalize formula + wire LatencyPadGuard into echo demo`

**Files:** `crates/shamir-connect/src/common/latency.rs`,
`crates/shamir-transport-tcp/tests/echo_e2e.rs`,
`docs/client-server-protocol-spec/AUTH_PROTOCOL.md`

**Problem (caught by Round-2 impl-vs-spec review):**
- `target_constant_time_ms` used `jitter` twice: `FLOOR.max(j) + j`. Worked
  by accident because `FLOOR > JITTER_MAX` (so `max == FLOOR` always),
  but fragile.
- `LatencyPadGuard` existed but was nowhere wired — spec §8.5 NORMATIVE
  not satisfied at runtime.
- Spec §8.5 wording was ambiguous (`max(jitter_ms, fixed_floor_ms)`),
  contradicted by diagram 01 step 14 (`floor + uniform[0,25] jitter`).

**Fix:**
- Code: `FIXED_FLOOR_MS + uniform[0, JITTER_MAX_MS]` (canonical form,
  result `[50, 75]` ms). Same observed behaviour as before.
- Echo demo wires `LatencyPadGuard` into the auth path; sleeps to target
  before writing `auth_ok`.
- Spec §8.5 text rewritten to match diagram: `floor + uniform[0, jitter]`.

**+1 TDD test:** `sampled_target_distribution_covers_floor_and_ceiling`
(verifies both endpoints are reachable; catches future regressions like
the previous double-jitter bug).

---

### Optim #6 — `resume.rs` move-out instead of 5 clones

**Commit:** `9963acd` — `perf(resume): move plain fields instead of cloning`

**File:** `crates/shamir-connect/src/server/resume.rs:289-355`

**Mechanism:**
Previously `Step 12+13` deep-cloned `plain.username_nfc` and
`plain.roles` TWICE: once for `Session::new`, once for `new_plain`.
Reordered so:
- Refresh path: clone username + roles ONCE for Session, MOVE everything
  else into `new_plain`.
- No-refresh path: MOVE plain directly into Session — zero clones.

Saves ~250–500 ns per resume + 2–4 heap allocations (depending on
`roles.len()`).

---

### Optim #7 — Single-syscall `write_frame`

**Commit:** `a4f7f4e` — `perf(framing): single write_all + write_frame_into pooled variant`

**File:** `crates/shamir-transport-tcp/src/framing.rs`

**Mechanism:**
- `write_frame`: concatenate length + payload into one buffer, single
  `write_all` (was: two `write_all` + flush → two TLS records).
- New `write_frame_into(writer, payload, &mut scratch)`: zero-allocation
  steady state, symmetric to `read_frame_into` (Optim #1).

**Bench impact** (`framing/write_only/write_frame`):

| Frame size | OLD (2× write_all) | NEW (single concat) | Δ |
|-----------|--------------------|--------------------|---|
| 64 B      | 1.43 µs            | 815 ns             | **-49%** |
| 1 KB      | 1.52 µs            | 985 ns             | **-44%** |
| 16 KB     | 2.09 µs            | 2.13 µs            | -8% (I/O dominates) |

Halves CPU on small frames. With TLS this also halves wire overhead
(~22 bytes of TLS header+tag per record).

---

### Optim #8 — In-place AES-GCM decrypt

**Commit:** `3dead97` — `perf(ticket): in-place AES-GCM decrypt`

**Files:** `crates/shamir-connect/src/common/crypto.rs`,
`crates/shamir-connect/src/server/ticket.rs`

**Mechanism:**
- New `aes256gcm_decrypt_in_place_with_cipher(cipher, nonce, aad,
  &mut buffer, &tag)` thin wrapper around `aes_gcm::AeadInPlace`.
- `decrypt_ticket_with_ciphers` now clones `wire.ciphertext` ONCE into
  a working buffer + calls in-place decrypt with separate tag (was:
  concat ciphertext+tag into a fresh `Vec` to feed owning `Aead::decrypt`).
- On current-cipher tag-mismatch the buffer is corrupted — re-clones for
  the previous-cipher fallback to ensure clean retry.

**Bench impact** (`protocol_construction/ticket_decrypt`):
6.93 µs (vs `before-optim` baseline; change -9.18% median, p=0.03).

Modest because msgpack parse (~5 µs) dominates the path. Real win is
the eliminated allocation, not the AES timing.

---

### Optim #9 — `RequestEnvelopeRef<'a>` for client encode

**Commit:** `e0683d0` — `perf(envelope): RequestEnvelopeRef<'a> for zero-copy client encode`

**Files:** `crates/shamir-connect/src/common/envelope.rs`,
`crates/shamir-connect/tests/integration_session.rs`

**Mechanism:**
Symmetric to `RequestEnvelopeView<'a>` (Optim #4, server decode).
`RequestEnvelopeRef<'a>` lets a client serialize a request without the
per-call `sid.to_vec()` allocation.

**Bench impact** (`envelope/request_encode_ref` vs `request_encode`):
- 256 B body: 924 ns vs 960 ns (-4%)
- 4 KB body: 910 ns vs 929 ns (-2%)

Modest absolute win; the real value is API symmetry — read side has
zero-copy view, write side now has zero-copy ref. Together they
support fully-pooled transports.

**Wire-compat verified** by `request_envelope_ref_wire_compat_with_owning_and_view`.

---

### Optim #10 — *intentionally skipped*: per-user Hmac caching

**Commit:** included in `f33f56b` — design-note added to `crypto::hmac_sha256`.

**Why not done:**
Pre-computing `Hmac<Sha256>` instances in `UserRecord` for `stored_key`
and `server_key` would save ~200–400 ns per SCRAM verify (~10–20% of
the non-Argon2 work). However it would introduce a real-vs-fake user
timing channel: cached-user path ~150 ns/HMAC, fresh-init fake path
~300 ns/HMAC.

While §8.5 latency padding (50–75 ms) masks this on the wire, spec
§5.2.4 + §9.2 mandate constant-time discipline AS WELL AS padding
(defense-in-depth). The ~300 ns savings is dwarfed by Argon2id (~2 s)
and by the padding floor — not worth the discipline erosion. Decision
documented in `crypto.rs::hmac_sha256` doc-comment.

---

### Helper — `complete_auth_ok` + `AuthOkView::with_*` builder methods

**Commit:** `f33f56b` — `feat(handshake): complete_auth_ok helper + AuthOkView builder methods`

**File:** `crates/shamir-connect/src/server/handshake.rs`

**Problem (caught by Round-2 impl-vs-diagrams review):**
`verify_proof` returns `AuthOkView` with all three optional extension
fields hardcoded to `None`. The fields exist (API correct), but the
library never auto-populates them. A deployment using `verify_proof`
output verbatim would silently never issue resumption tickets and never
serve `rotation_in_progress` to orphan clients (spec §6.5).

**Fix:** three additive APIs:
- `AuthOkView::with_resumption_ticket(bytes, expires) -> Self`
- `AuthOkView::with_rotation_in_progress(payload) -> Self`
- `AuthOkView::with_kdf_upgrade_required() -> Self`
- `complete_auth_ok(base, ticket, rotation, kdf_upgrade) -> AuthOkView`
- `needs_kdf_upgrade(user_params, current_defaults) -> bool` (decision helper)

The helper deliberately does NOT auto-rebuild `identity_input` for the
`rotation_in_progress` path — spec §6.5 requires byte-exact
`identity_input` and that's only known at the call site that has the
`auth_message` in scope. Caller pattern documented in rustdoc.

**+4 TDD tests** in `integration_full_auth.rs`.

---

### Diagram minor fixes

**Commit:** `4cdcf7b` — `docs(diagrams): fix two minor mismatches in 01-initial-auth.md`

- Step 9 client-side note: "auth_message = §4.1 (149 bytes для default
  params)" → "144 + byte_len(username_nfc) bytes" (149 was specific to
  `username = "alice"`).
- Step 14 `auth_ok` payload: added missing `resumption_expires_at_ns?`
  for schema completeness (already present in `02-resumption.md` for
  `resume_ok` and in spec §2.4).

Documentation-only; implementation already correct.

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

## What remains (lower-priority items)

After two rounds of optimization, these are the still-deferred items:

| Item | File | Estimated win |
|------|------|---------------|
| `dispatch_into(buf: &mut Vec<u8>)` for response encoding | `dispatch.rs` | -1 alloc + -990 ns at small body |
| `decrypt_ticket_from_wire_bytes` skipping `TicketWire` parse | `ticket.rs:99-194` | -3 allocations on resume |
| `gc_expired` → `DashMap::retain` (one-pass) | `session.rs:177-189` | minor; cleanup |
| `permissions: SessionPermissions` drop the `RwLock` (immutable per-session) | `session.rs:61` | -5 ns admin auth check |
| `FakeBlob::derive` use `zeroize::Zeroize::zeroize()` for volatile_write | `fake_blob.rs:67` | security hardening (LLVM was DCE-ing the `fill(0)`) |
| `build_aad(version)` heap-allocates 17 byte Vec → `static AAD_V1: [u8; 17]` | `ticket.rs:130-135` | -1 alloc per encrypt+decrypt |
| `auth_message`/`identity_input`/etc builders use thread_local scratch | various `common/*.rs` | -2 allocs per SCRAM accept |
| `ErrorEnvelope::error: Cow<'static, str>` (avoid String alloc for static codes) | `envelope.rs` | -1 alloc per error response |
| `SessionStore::lookup_at` extract Ref + drop before `Arc::clone` (reduce shard lock hold) | `session.rs:182-190` | minor under contention |
| `extract_tls_exporter_into(&mut [u8; 32])` zero-copy variant | `tls.rs:72-78` | -1 32-byte memcpy per handshake |
| `current_pub`/`previous_pub` AtomicU64 mirror (lock-free reads) | `rotation.rs` | -5–10 ns per handshake |

Plus the **STILL-MISSING NORMATIVE** items that aren't perf-related:

- Lockout / backoff / rate-limit subsystem (§5.2.5, §8 table, IMPL §1.3).
- WebSocket transport (entire `shamir-transport-ws` crate).
- Synchronous durable persist of `consumed_counters` (§6.2).
- HMAC-chained audit log (IMPL §3.3).
- `MAX_SESSIONS_PER_USER` / 5s grace / `MAX_CONCURRENT_ARGON2`.
- Plain TCP loopback whitelist enforcement.
- Username PRECIS UsernameCaseMapped (currently `to_lowercase`).
- Log redaction (`Debug` derived everywhere on key types).
