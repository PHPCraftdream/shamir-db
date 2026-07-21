# Changelog

All notable changes to ShamirDB will be documented here.

The project is currently in alpha and has not published a stable release. Until the first tagged release, the `master` branch is the primary development line and compatibility is not guaranteed.

## Versioning scheme

During the alpha phase this project uses `MAJOR.MINOR.PATCH-alpha.N` (for example `0.1.0-alpha.1`). The `alpha.N` suffix is a SemVer pre-release identifier — incrementing `N` does **not** imply any compatibility guarantee between two alpha releases.

> ⚠️ **Alpha-to-alpha compatibility: NOT guaranteed.** The on-disk storage format, the wire protocol, and the public API (Rust, TypeScript, and WASM) MAY change incompatibly between any two `0.1.0-alpha.N` releases. There is **no supported in-place upgrade path** between alpha versions yet — upgrading between alphas may require an **export/import cycle** (dump data with the old version, re-import with the new one). Treat each alpha as throwaway and keep independently verifiable backups. This is the single most important operational fact for an early adopter.

This file follows [Keep a Changelog](https://keepachangelog.com) conventions loosely and will adhere to [Semantic Versioning 2.0.0](https://semver.org) pre-release semantics once stable releases begin.

## [Unreleased]

- Ongoing development of the Rust workspace, server, clients, storage backends, query engine, authenticated transports, and WASM integration points.
- Release infrastructure: version all workspace crates at `0.1.0-alpha.1`, add `publish = false` to every crate as an accidental-publish safety net, and introduce this CHANGELOG with the alpha versioning scheme and the no-alpha-to-alpha-compatibility statement above.
- Resource-safety defaults (RI-8): tighten the code-level defaults that applied when a config omits the corresponding field — `security.query_limits.max_result_size_bytes` 1 GiB → 64 MiB (a single batch response is now clamped to 64 MiB unless the operator raises it explicitly) and `security.connection.max_active_connections` 10000 → 1000. Operators relying on the implicit 1 GiB / 10000 values without an explicit `security` block get the new lower caps on next deploy; the existing `server.example.ktav` and the two new resource profiles (`server.small.example.ktav`, `server.medium.example.ktav`) set these fields explicitly and are unaffected by the default change.
- Bootstrap-token lifecycle (RI-9): new `--bootstrap-token-path <PATH>` CLI flag to override the default `data_dir/bootstrap_token.txt` output location (recommended: a tmpfs path so `backup --to` never captures it). The bootstrap token now auto-deletes itself — on the first successful login for the bootstrap username, or after a 24h TTL boot-time sweep, whichever comes first — instead of relying on the operator to delete it manually.
- **Replication behavioral change (RI-10):** the leader now rejects a follower `Hello` whose `proto_ver` exceeds this build's `CURRENT_REPL_PROTO_VER` (`proto_ver_unsupported`) instead of accepting any version unconditionally. More importantly — **a follower that hits a journal gap now STOPS instead of silently skipping the missing range.** Previously, when the leader reported `gap_at` past the follower's requested `from_version`, the follower loop logged a warning, shifted its cursor past the gap, and kept running with permanently missing data and no visible signal. After this change the loop terminates and the subscription's row in `system/subscriptions` is marked `state = "resync_required"` (visible via the existing `ReplicationStatus`/`ListSubscriptions` admin responses); `reconcile()` will not restart a subscription in this state. **Operators who previously relied on the old keep-running-anyway behavior will see affected subscriptions come up stopped after upgrading** — recover by verifying/fixing the follower's data out of band, then issuing the existing `Resume` admin action. Full automated snapshot-based reseed remains roadmap (R2). Leader-follower replication is now explicitly documented as Experimental (`docs/guide-docs/guide/08-interconnect.md`).
