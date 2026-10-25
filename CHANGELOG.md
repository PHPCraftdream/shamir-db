# Changelog

All notable changes to ShamirDB will be documented here.

The project is currently in alpha and has not published a stable release. Until the first tagged release, the `master` branch is the primary development line and compatibility is not guaranteed.

## Versioning scheme

During the alpha phase this project uses `MAJOR.MINOR.PATCH-alpha.N` (for example `0.1.0-alpha.1`). The `alpha.N` suffix is a SemVer pre-release identifier — incrementing `N` does **not** imply any compatibility guarantee between two alpha releases.

> **Workspace-version scope (Node client — documented gap, F-52):** the release pipeline's `version-consistency` job (`.github/workflows/release.yml`) verifies that every *Cargo* workspace crate's `version = "..."` agrees with the pushed `v*` tag. It deliberately does **not** cover `crates/shamir-client-node/package.json`'s `"version"` field: that crate is excluded from the Cargo workspace (`Cargo.toml` `exclude` — napi-rs cdylib, MSVC-only, built separately per `CLAUDE.md`), uses a different manifest format (Node `package.json`, never matched by the `crates/*/Cargo.toml` grep), and is versioned on its own cadence. Its `"version": "0.1.0"` therefore legitimately differs from the workspace's `0.1.0-alpha.1` — this is a documented scope gap, **not** a bug to "fix" by bumping it. There is currently no separate npm-publish workflow for it either (only `ts-e2e-nightly.yml`'s `node-e2e` job builds/tests the `.node` binding locally; no publish step exists yet); adding one is a separate, larger task.

> ⚠️ **Alpha-to-alpha compatibility: NOT guaranteed.** The on-disk storage format, the wire protocol, and the public API (Rust, TypeScript, and WASM) MAY change incompatibly between any two `0.1.0-alpha.N` releases. There is **no supported in-place upgrade path** between alpha versions yet — upgrading between alphas may require an **export/import cycle** (dump data with the old version, re-import with the new one). Treat each alpha as throwaway and keep independently verifiable backups. This is the single most important operational fact for an early adopter.

This file follows [Keep a Changelog](https://keepachangelog.com) conventions loosely and will adhere to [Semantic Versioning 2.0.0](https://semver.org) pre-release semantics once stable releases begin.

## [Unreleased]

First public alpha in preparation (`0.1.0-alpha.1`, not yet tagged). The
detailed engineering log previously kept here lives in
`docs/dev-artifacts/checkpoints/2026-08-engineering-log.md`.

### Changed (index rebuild required)
- Functional index posting hashes: computed `Dec`/`Big`/`Bin`/`Set` values now hash by content (previously collapsed to one shared tag-255 hash, RV-INDEX-56A505F0). Existing functional indexes hold postings under the old hashes and MUST be dropped and re-created after upgrading.
- FTS tokenizers (Whitespace, Full): non-ASCII uppercase words (e.g. Russian/Greek) are now case-folded before hashing (RV-INDEX-A570106D). Existing FTS indexes contain pre-fix token hashes and MUST be dropped and re-created after upgrading.
- Legacy regular/unique/functional index posting hashes for numeric (Int/F64) keys are canonical now — `Int(1)` and `F64(1.0)` share one key, `-0.0` keys as `0`, NaN is never matched by an equality probe nor a unique conflict (RV-FUNCLIB-5FA455F7). Existing indexes holding numeric keys MUST be dropped and re-created after upgrading (alpha: no in-place upgrade path).

### Included
- Single-node server (`shamir-server`): TCP, WebSocket and local IPC listeners; TLS 1.3 + SCRAM-Argon2id; session resumption; rate limiting and lockout.
- Document storage on Fjall with WAL-based crash recovery; MVCC transactions (snapshot, serializable, pessimistic), single-batch and interactive.
- Batch query API built with the Rust and TypeScript query builders: filters, projections, aggregates, ordering, pagination, cross-query references, conditional execution, loops, cursors.
- Indexes: hash, unique, sorted/covering, FTS (BM25, Snowball stemming), functional, vector (HNSW, optional SQ8).
- WASM functions, procedures and validators (upload pre-built `.wasm`); Shomer access control; changefeed; live subscriptions.
- Offline `backup` / `restore` CLI with a sha256 manifest; offline `doctor`.

### Experimental
- Leader→follower async read replicas (schema not replicated, no automatic reseed, no failover).
- Storage-engine migration API — disabled by default, known data-loss gaps.
- Server-side Rust→WASM compilation — development only.

### Known limitations
See `docs/guide-docs/KNOWN_LIMITATIONS.md`. No online backup, no point-in-time recovery, no in-place upgrade between alpha versions, no published binaries or packages.
