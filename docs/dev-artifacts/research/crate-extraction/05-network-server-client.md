# Crate-extraction research — batch 5: network, server, client crates

**Date:** 2026-07-16
**Scope:** `shamir-connect`, `shamir-server`, `shamir-transport-tcp`,
`shamir-transport-ws`, `shamir-client`, `shamir-sdk`, `shamir-client-ts` (TypeScript)
**Question:** what, if anything, inside these crates should be extracted further —
(a) into a workspace-internal crate for testability/isolation, or (b) into a
published crates.io / npm package (the `bench-scale-tool` precedent).

**Method:** read of each crate's `Cargo.toml`, full `src/` tree LOC listing, and
the source of every candidate module. No code was modified.

---

## Verdict at a glance

| Candidate | Source | Proposed package | Verdict |
|---|---|---|---|
| HMAC-chained tamper-evident audit log | `shamir-connect/src/server/audit_chain.rs` | `hmac-audit-chain` | **Strongest candidate** — genuinely generic, often reinvented, small dep footprint |
| Auth brute-force-defence kit (lockout + subnet rate-limit + KDF semaphore + latency padding) | `shamir-connect/src/server/{lockout,rate_limit,argon2_semaphore}.rs` + `common/latency.rs` | `auth-hardening` (or split) | **Good candidate** — coherent, reinvented everywhere, moderate detangling needed |
| SCRAM-Argon2id protocol core | `shamir-connect/src/common/{scram,crypto,auth_message,kdf_params,fake_blob,username}.rs` + client/server handshake | `scram-argon2id` | **Borderline** — self-contained, but it is a bespoke (non-RFC-5802-wire) protocol |
| Length-prefixed async framing | `shamir-transport-tcp/src/framing.rs` | `lp-frame32` | **Weak** — clean and fast, but `tokio_util::codec::LengthDelimitedCodec` owns this niche |
| RFC 9266 TLS-exporter helper | `shamir-transport-tcp/src/tls.rs` (`ConnectionExporter`) | — | **Too small alone**; ship inside whatever protocol crate leaves, if any |
| Lock-free runtime log-mask + batched file writer | `shamir-server/src/logging.rs` | `tracing-livemask` | **Weak-moderate** — real differentiator vs `tracing_subscriber::reload`, but niche |
| `@shamir/client` TS package | `crates/shamir-client-ts` | npm `@shamir/client` | **Publish as product**, not an extraction; no generic sub-package inside |
| `shamir-sdk` | `crates/shamir-sdk` | crates.io `shamir-sdk` | **Publish as product** eventually (function authors need it); nothing generic inside |
| Everything else in scope | — | — | Not worth extracting (reasons per crate below) |

---

## 1. shamir-connect (~7,500 LOC src, excl. tests)

**What it is.** The transport-agnostic connection/auth protocol library:
SCRAM-with-Argon2id handshake, Ed25519 server identity + pinning, AES-GCM
resumption tickets, session store with post-auth rate limiting, per-subnet
`auth_init` rate limiting, lockout/backoff, HMAC-chained audit log, key/identity
rotation, password change, bootstrap, admin ops. Feature-split into `common` /
`client` / `server`. Notably it is **already almost a leaf crate**: its only
shamir dependency is `shamir-tunables`, used in exactly two lines of
`server/session.rs` for one constant (`POST_AUTH_RATE_LIMIT_PER_SEC`). Every
other dependency is an external crypto/collections crate.

This crate is where all the extraction value in this batch lives. Three
candidates, in descending order of conviction:

### 1a. `server/audit_chain.rs` → `hmac-audit-chain` — STRONGEST CANDIDATE

**Scope:** 444 LOC + tests in `server/tests/`. An append-only structured audit
log where `entry.hmac = HMAC-SHA256(key, prev_hmac || canonical_bytes(entry))`,
with a periodic `last_audit_hmac` checkpoint as truncation defence and a
startup chain verifier.

**Dependency footprint:** `hmac`, `sha2` (via the crate's own thin
`hmac_sha256` wrapper — trivially inlined), `parking_lot::Mutex` for the
appender tail, `serde`/`rmp-serde` for the details payload. Zero shamir-*
dependencies. The `AuditEntry` fields (`transport`, `ip_subnet`,
`session_id_prefix`) are shamir-flavoured; a generic crate would either keep a
fixed canonical layout with generic string fields (they already are plain
strings) or make the canonical-bytes encoder a trait.

**FOR.** Tamper-evident hash/HMAC-chained logs are reinvented constantly
(compliance logging, security event trails, "blockchain-lite" audit
requirements) and are easy to get subtly wrong — truncation defence, canonical
byte encoding, and constant-time final-HMAC comparison are exactly the parts
people botch. This implementation has all three, is documented against a spec,
has a deterministic cross-language canonical form, and its state surface is a
single `Mutex<(seq, prev_hmac)>` — small enough to publish and maintain
honestly. There is no dominant crates.io incumbent for "HMAC-chained audit log
with checkpointed truncation defence" the way there is for framing or SCRAM.

**AGAINST.** The canonical byte layout (`u64_be(seq) || ... || prev_hmac(32)`)
is normative to shamir's protocol spec; publishing it as-is exports a
shamir-specific wire layout as if it were a general standard, and generalising
the layout (pluggable encoder) removes the "byte-identical across
implementations" guarantee that is the module's main discipline. Also the
persistence side (checkpoint sink, startup verification driver) lives in
`shamir-server/src/audit_appender.rs` (574 LOC) and is entangled with
`server_meta` — a published crate would ship the chain math but each user
still writes their own storage glue, shrinking the delivered value to ~300
useful LOC. Net: worth doing if the maintainer wants a second published crate;
not urgent for shamir itself since the module is already cleanly isolated and
unit-tested in place.

### 1b. Brute-force defence kit → `auth-hardening` — GOOD CANDIDATE

**Scope:** ~1,100 LOC across four files, all with in-place unit tests:

- `server/lockout.rs` (479 LOC) — per-`(subnet, username_hash)` exponential
  backoff (100ms × 2^N, cap 30s) + silent hourly lockout, HMAC-keyed username
  hashing, pluggable `LockoutStore` trait, snapshot persistence with a
  documented secure-direction rehydration policy.
- `server/rate_limit.rs` (376 LOC) — per-/24-or-/64-subnet token bucket with a
  post-restart warmup divisor and the same conservative snapshot rehydration
  (restore tokens verbatim, reset refill clocks — attacker gains nothing from
  inducing a restart).
- `server/argon2_semaphore.rs` (141 LOC) — atomic counting semaphore capping
  concurrent memory-hard KDF invocations (Argon2id ≈128 MB each → OOM DoS
  without it). Runtime-agnostic (Condvar-based, `try_acquire` for async).
- `common/latency.rs` (101 LOC) — constant-time response padding
  (`floor + uniform jitter`, pure-function core, RAII guard, runtime-agnostic).

**Dependency footprint:** `dashmap` + `rustc-hash`, `serde`, `hmac`/`sha2`,
`rand`, plus two tiny internal modules that would move along: `common/time.rs`
(64 LOC UnixNanos helpers) and the `Subnet` type. Zero other shamir deps.

**FOR.** This is precisely the "protocol/security infrastructure that's often
reinvented" category. Every self-hosted service that does password auth needs
some subset of: KDF concurrency cap, per-subnet rate limit, per-user backoff +
lockout, timing-equalised responses — and there is no coherent crates.io kit
for it (`governor` covers generic rate limiting but not the
subnet-granularity/lockout/warmup/snapshot-rehydration policy bundle, and
nothing covers the KDF semaphore + latency padding pairing). The
security-reasoned snapshot rehydration (documented insecure-vs-secure
direction) is the kind of hard-won detail that makes a published crate
valuable. The code is already trait-pluggable (`LockoutStore`, `RateLimiter`,
snapshot sinks) and tested.

**AGAINST.** The constants are normative to shamir's spec (10/sec, /24, 50
fails/hour, 50–75 ms padding) — a general crate must make them config, which
is easy, but then the crate is "opinionated defaults + policy engine" and
needs real docs to be usable safely; a security crate published casually and
half-maintained is worse than none. The four pieces are also independently
small — a skeptic would say `lockout.rs` alone is the only nontrivial one.
And shamir gains little internally: the modules are already isolated,
feature-gated, and unit-tested. This is a community-value play, not a
testability play. Recommended only if the maintainer is willing to own it as
a real published project (à la `bench-scale-tool`).

### 1c. SCRAM-Argon2id core → `scram-argon2id` — BORDERLINE, be honest about it

**Scope:** `common/scram.rs` (123 LOC of key derivation / proof arithmetic),
`common/crypto.rs` (384 LOC of pinned primitive wrappers), `auth_message.rs`,
`kdf_params.rs`, `fake_blob.rs` (anti-enumeration fake verifier),
`username.rs` (RFC 8265 PRECIS wrapper), plus `client/handshake.rs` (311 LOC)
and `server/handshake.rs` (390 LOC). Roughly 1,500 LOC for a full
"SCRAM-Argon2id over an arbitrary transport" library.

**FOR.** RFC 5802 SCRAM is defined over PBKDF2/SHA-x; everyone who wants SCRAM
semantics with a memory-hard KDF ends up rolling their own variant (the
existing `scram` crate is SHA-1/SHA-256 textual GS2 only). This implementation
is transport-agnostic by construction, constant-time-disciplined (fake-blob
branch equivalence, `subtle` comparisons, zeroization throughout, redacted
Debug impls), carries wire test vectors, and its only workspace coupling is
the two-line tunables constant noted above. Extraction cost is genuinely low.

**AGAINST — the decisive point.** This is not standard SCRAM: msgpack binary
envelopes, shamir-specific domain-separation tags (`common/domain_tags.rs`),
spec-§ references on every function, Ed25519 identity pinning and channel
binding baked into the auth_message layout. Publishing it means asking the
ecosystem to adopt *shamir's protocol*, not giving them a building block for
theirs — interop value is zero unless they also run a shamir-shaped server.
The honest options are: (i) publish the whole `shamir-connect` under its own
name as "the ShamirDB connection protocol, reusable if you like it" (cheap,
low reach), or (ii) do real design work to parameterise domain tags/envelopes
into a generic SCRAM-framework crate (expensive, speculative). Neither is
compelling today. **Recommendation: defer**; revisit if/when shamir-db itself
is published and external clients need the crate anyway — at that point
publishing `shamir-connect` as-is becomes necessary product work, not
extraction.

**Everything else in shamir-connect** (session store, resume/ticket, rotation,
changepw, bootstrap, admin, dispatch, envelope, durable counters) is
protocol-specific state machinery — correctly placed, not extraction material.

---

## 2. shamir-transport-tcp (549 LOC)

**What it is.** TLS 1.3 wiring (rustls server/client config builders, rcgen
self-signed certs, no-CA verifier for Ed25519-pinned identity), RFC 9266 TLS
exporter extraction, a TCP listener, and the length-prefixed framing layer.

### `framing.rs` → `lp-frame32` — WEAK

**Scope:** 259 LOC, zero shamir deps in-file (tokio + thiserror only), with a
dedicated bench (`benches/framing.rs`) and unit tests. `[u32_be len][payload]`
with a zero-length graceful-close sentinel, a 16 MiB cap, and a family of
carefully-optimised variants: alloc-per-call, buffer-reuse (`read_frame_into`
with a documented `unsafe set_len` fast path), single-TLS-record combined
writes, and a pre-reserved zero-copy write path.

**FOR.** It is exactly the shape of thing the brief asks about: generic,
dependency-light, benchmarked, documented. Extraction cost ≈ an afternoon.

**AGAINST.** `tokio_util::codec::LengthDelimitedCodec` is the entrenched
ecosystem answer to length-prefixed framing, is far more configurable, and is
maintained by the tokio org. The genuine deltas here — zero-len-as-close
semantics, the TLS-record-coalescing write, the prereserved-buffer write —
are optimisations tied to shamir's request loop, not features a general user
would pick this crate for over tokio-util. A published crate would be a
me-too with a maintenance obligation. **Not recommended for crates.io.** As a
workspace crate it is already minimal and well-isolated; nothing further to do.

The `ConnectionExporter` trait + `extract_tls_exporter` (RFC 9266 keying
material over tokio-rustls client/server streams, ~50 LOC) is a neat utility
that people do re-derive, but it is too small to publish alone and belongs
with whatever protocol crate would consume it (see 1c). The no-CA verifier is
a footgun outside its pinned-identity context and should not be published.

---

## 3. shamir-transport-ws (586 LOC)

**What it is.** WSS bindings: the same `[u32_be][payload]` frame carried in
WebSocket BINARY messages (with a redundant inner length check as
defence-in-depth), split-halves send/recv helpers over tungstenite, a WSS
listener with Origin inspection, browser-mode (`binding_mode=0x02`, zeroed
channel binding) vs native-mode (0x01, real TLS exporter) distinction, and a
25-line `tls_exporter.rs` shim re-exporting the TCP crate's extractor.

**Verdict: nothing worth extracting.** Every module is a thin, spec-coupled
adapter between tungstenite and the shamir framing/binding conventions. The
double-length-prefix scheme only makes sense given shamir's cross-transport
frame format. It is already a small, testable crate.

---

## 4. shamir-server (~16,300 LOC)

**What it is.** The deployable server binary: wires shamir-connect auth +
both transports + shamir-db into a connection loop; owns config, user
directory, server meta/persistence, subscriptions bridge, replication
supervisor, scheduler, observability, backup, Windows service integration.
By nature this is integration code — nearly all of it is correctly
non-extractable. Modules examined for generality:

### `logging.rs` → `tracing-livemask` — WEAK-MODERATE

**Scope:** 424 LOC + tests. Two things: (1) a batched-file non-blocking
writer (bounded MPSC → single worker thread → `BufWriter`, timed + threshold
flush, loss-free shutdown drain); (2) a lock-free runtime-adjustable
per-namespace level mask stored in a global `ArcSwap` — the hot-path
`enabled()` check is one atomic load + longest-prefix match, explicitly
avoiding `tracing_subscriber::reload`'s internal `RwLock`.

**FOR.** The lock-free live mask is a real differentiator: `reload::Layer`'s
RwLock on the filter hot path is a known sore point for high-throughput
tracing users, and "change log levels at runtime without a lock" is a
recurring ask. Dep footprint is small (`arc-swap`, `tracing-subscriber`,
`once_cell`).

**AGAINST.** It is entangled with `crate::config::LoggingConfig` and shamir's
namespace taxonomy (`ns::WAL` etc.); the file-writer half re-implements what
`tracing-appender` mostly covers; and the tracing ecosystem is crowded with
half-maintained layer crates — standing out requires benchmarks and docs the
maintainer may not want to own. Internally there's no isolation problem: it
has its own tests. **Recommendation: not now; harvest the LogMask idea into a
crate only if there's appetite for a tracing-ecosystem contribution.**

**Explicitly considered and rejected:**

- `conn_limiter.rs` (249 LOC) — atomic cap + RAII guard + per-IP map. Correct
  but trivial; a published crate would be ~50 lines of value.
- `framer.rs` (439 LOC) — the `Framer` trait unifying TCP-TLS and WS framing
  is good internal architecture but is defined *by* the two shamir transports;
  generalising it is designing a new abstraction, not extracting one.
- `scheduler.rs` (330 LOC) — periodic-GC driver; every type it touches is a
  shamir-connect store. Domain glue.
- `windows_service.rs` (220 LOC) — thin adapter over the `windows-service`
  crate, hardcoded to shamir's config/boot path.
- `user_directory.rs`, `server_meta.rs`, `audit_appender.rs`, `access_tree.rs`,
  `subscriptions/*`, `replication/*`, `db_handler/*`, `observability.rs` — all
  deeply shamir-db-coupled (system tables, wire types, engine handles).

No testability-driven splits are needed either: the crate already follows the
tests-per-module layout and its heaviest module (`server_launcher.rs`,
1,325 LOC) is launch wiring that only integration tests can meaningfully cover.

---

## 5. shamir-client (~2,200 LOC)

**What it is.** The async Rust client: TLS+SCRAM connect/resume, a
rid-demultiplexing background reader (register-oneshot-before-send pattern,
concurrent in-flight requests), subscription push routing with early-frame
buffering, and a lock-free client-side mirror of the server's field-name
interner (`interner_cache.rs` + `interner_cache_ops.rs`, ~970 LOC — scc maps,
CAS-max epoch, OnceCell dump-stampede guard).

**Verdict: nothing worth extracting.** The rid-demux multiplexer is the one
pattern with generic appeal, but it is ~150 effective LOC interleaved with
shamir envelopes, push frames, and resume logic; the generic ecosystem answer
(`tower`, or hand-rolled oneshot maps) is well-trodden. The interner cache is
clever but only meaningful against shamir's append-only interner protocol.
The crate is already thin and its modules are unit-tested in place. Plainly:
no.

---

## 6. shamir-sdk (~1,430 LOC)

**What it is.** The guest-side authoring SDK for user WASM functions:
`#[shamir_sdk::function]`/`#[procedure]`/`#[scalar]`/`#[validator]` macro
surface, host-import shims, `Ctx`/`Params`/`Value` types, a small validation
DSL, HTTP-from-guest types, and optional in-guest query-builder re-export.

**Verdict: nothing to extract, but note the publishing angle.** Internally
there is no generic component — everything encodes shamir's WASM ABI. However,
this crate (with `shamir-sdk-macros` and `shamir-query-builder`) is the thing
third-party function authors must depend on, so *it* is the next natural
crates.io publication as **product** work once the ABI stabilises — the same
way `@shamir/client` must eventually reach npm. That is a release-engineering
task (version discipline, ABI stability guarantees), not an extraction.

---

## 7. shamir-client-ts (TypeScript, ~10,000 LOC src excl. tests)

**What it is.** The full TS client for Node + browser: WS transport with the
inner length-prefix framing (`core/framing.ts`), SCRAM-Argon2id via
argon2-browser + @noble/* (`core/scram.ts`, 204 LOC), the HMAC intent-guard
mirror (`core/hmac.ts`, 408 LOC), field-map interner mirror, platform
abstraction (Node `ws` vs browser WebSocket), and ~3,500 LOC of typed query /
DDL / batch / filter / admin builders mirroring `shamir-query-builder`.
Currently `"private": true`, published nowhere.

**Verdict: publish the package itself; no generic sub-extraction.**

**FOR (publishing `@shamir/client` to npm).** It is the product's client SDK —
if shamir-db is meant to be used by anyone, this must be on npm; it already
has clean package structure (dual Node/browser exports, dist build, vitest
suite, its own README + TRANSPORT_SPEC.md). The effort is release engineering
(scope name, CI publish, semver policy), not code work.

**AGAINST sub-extraction.** Every module that looks generic is thin glue over
already-published npm libraries: `scram.ts` composes @noble/hashes +
argon2-browser (the crypto heavy lifting is upstream); `framing.ts` is ~170
LOC of protocol-specific byte handling; `hmac.ts` is normative shamir wire
layout; the builders are the domain itself. The JS ecosystem also already has
SCRAM packages for the standard variants. Nothing here would earn independent
npm adoption.

One hygiene note: as with the Rust side, the SCRAM/HMAC/framing modules are
byte-mirrors of Rust files (`hmac.ts` explicitly names
`crates/shamir-query-types/src/hmac.rs` as its twin). Any future extraction of
1c on the Rust side would need to keep this mirror pair in lockstep — an
argument *for* keeping the protocol core inside the workspace where the
cross-language vector tests live.

---

## Recommendations (ranked)

1. **`hmac-audit-chain`** from `shamir-connect/src/server/audit_chain.rs` —
   the one clean crates.io candidate in this batch: generic problem, no strong
   incumbent, ~450 LOC, deps = `hmac`/`sha2`/`parking_lot`/`serde`. Decide
   whether the canonical byte layout stays fixed (simplest, honest "our
   layout" crate) or becomes pluggable.
2. **`auth-hardening`** (lockout + subnet rate-limit + KDF semaphore + latency
   padding) — high community value, ~1,100 LOC, needs constants-to-config work
   and a commitment to maintain a security-adjacent crate. Do it only with
   that commitment.
3. **Defer** the SCRAM-Argon2id core: publish `shamir-connect` as-is when the
   product ships to external users; do not generalise it speculatively.
4. **Do not publish** the framing layer (`tokio-util` owns the niche) or the
   server's logging/conn-limiter/framer modules.
5. **Product publishing backlog** (separate from extraction): npm
   `@shamir/client` and crates.io `shamir-sdk`(+macros, +query-builder) are
   the packages the outside world will actually need first.

Nothing in this batch has a *testability* problem that extraction would fix —
all seven crates already follow the per-module `tests/` discipline and are
individually compilable; the extraction case everywhere is community reuse,
not isolation.
