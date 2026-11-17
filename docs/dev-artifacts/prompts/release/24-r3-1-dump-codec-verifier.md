# Brief: R3 PR-1 — logical-dump codec and verifier (release-1 R3, implementation)

Design: `docs/dev-artifacts/design/logical-dump.md` — read it in full, the "Review" section included. This is PR 1 of its
implementation plan (section 6). Sections 0, 3 (framing, canonical named MessagePack, logical values, portable content) and
5 (tests) are binding for this step.

## Why

Release blocker R3: a portable logical dump is the fallback when an in-place format migration is unavailable and the way to
move a database between machines. PR-1 builds the self-contained artifact layer — writer, reader and verifier — with no
database access. Export (PR-2) and import (PR-3) build on it.

## What

1. **Framing** (design 3.1): magic `SDBLDMP\n`, `version:u16`, `flags:u16`, `header_length:u32`, header + its SHA-256,
   checksummed frames (`SHA256(sequence:u64 || length:u32 || payload)`, implicit sequence), section-start/item/section-end
   records with counts and section digests, the trailer frame with totals and the prefix digest, EOF right after it.
   Fixed section order exactly as listed in 3.1 item 4.
2. **Limits** (3.1): 16 MiB frame and header, 64 MiB aggregate metadata, 64 MiB working buffer, depth 64, 65,536 entries per
   container, 1 TiB artifact (file preflight). Enforce before allocation. Unknown required features or nonzero unsupported
   flags fail closed.
3. **Canonical named MessagePack and the tagged value envelope** (3.2): sorted format maps, insertion order kept for record
   `Map`/`Set` values, explicit tags distinguishing `Int`/`Dec`/`Big`/`Str`, fixed-width integers, exact IEEE-754 bit
   patterns, bytes for binary, rejection of invalid UTF-8, duplicate names, unknown mandatory fields, noncanonical
   encodings and unsupported kinds. Raw record key bytes preserved; records sorted by raw key bytes.
4. **Shared helpers** (3.1 last paragraph): the canonical named-MessagePack encode/decode helpers, the limits and the
   SHA-256 helpers are shared with the replication seed codec
   (`crates/server/shamir-sync/src/sync/replication_seed/artifact/encoding/`), not a third copy. Reuse what exists; where
   the seed codec's helper must be generalised, do it without changing the seed's wire format or behaviour (its tests must
   stay green) and say so in the report.
5. **Streaming writer, reader and verifier**: bounded memory, forward-only; the verifier checks every frame, count, section
   digest, the trailer and EOF, and reports the first failure precisely. A typed writer API produces the sections in order;
   a reader yields typed items. No database access in this PR.
6. **Reference fixture**: a small deterministic artifact covering every logical value kind and every section, checked in as
   test data, and the seed reference fixture comparison the design names.
7. **Tests** (CI only): round trip of every value kind (signed zero, NaN payloads, `Dec` vs `Str`, insertion-ordered maps);
   byte determinism; every corruption, truncation, trailing-data, over-limit, unsupported-feature, noncanonical and
   duplicate case of 3.1–3.2; the 36-byte-per-item-frame overhead assertion; the 1 TiB preflight without allocating.

## Rules

* No new dependency. No encryption (design decision 9). No change in `shamir-engine` or any engine crate.
* Placement: the design counts `crates/server/shamir-sync/src/` and `.../sync/` at 7 entries (2026-11). Count again from a
  listing and choose the home (a new subfolder under an existing one with room, or a regroup as its own preparatory change,
  listed separately in the report so the orchestrator commits it apart). Name the crate and every touched folder with its
  counted entries in the report.
* At most 7 entries per folder and 1000 lines per file; one primary export per file; `mod.rs` re-exports only; no local
  `use`; no inline test modules; no new `#[allow]`; no visibility widening.

## Acceptance (observed in CI)

fmt, `cargo clippy -p <touched crates> --all-targets -- -D warnings` and layout clean locally; the touched crates' tests
green in CI (the seed codec tests included). The final report lists every file created and changed, every design point
implemented and every deviation with its reason.
