# Brief: R2a step 2 — online copy engine in `shamir-sync` (release-1 R2a, implementation)

Design: `docs/dev-artifacts/design/online-backup.md` — read it in full, the "Review" section included. This is step 2 of
its implementation plan (section 7). Sections 4.1 (placement), 4.2 (protocol), 4.3, 4.5 (manifest v2), 4.6 (layout) and
6 (tests) are binding for this step. Step 1 (the engine facade `capture_point` / `seal_point` / `replay_to`) is built in
parallel by another change and is NOT available to you.

## Why

Release gate: "a backup is taken without stopping the server, restores on a clean machine, and a test under write load
proves it". Step 2 builds the copy machinery as a pure library; step 3 wires it into the server.

## What

1. **Regroup first (its own preparatory change).** Move `sync/durability/backup.rs` and `sync/durability/restore.rs` into
   a new folder `sync/durability/backup/` (design 4.1), keeping every existing public path valid through re-exports and
   moving their tests with them. No behaviour change. List these paths separately in the report: the orchestrator commits
   the regroup apart from the feature.
2. **Consistency-point port.** The engine is not reachable from here yet. Define a small trait in the new module — the
   port the copy engine calls at steps A, B, E and G of the protocol (acquire and release the truncation pin, capture a
   repository's flushed position, seal it) — with exactly the data design 4.2 names (`wal_position` per repository, the
   system-store version, the consistency point `V*`). Step 1 implements it in the engine; step 3 connects the two. Keep the
   trait minimal and document each method with the protocol step it serves.
3. **Protocol state machine** (design 4.2): `Preparing → Captured → Copying → Sealing → Verifying → Done | Failed(stage)`,
   steps A to G in order, WAL copied before stores, the second hash pass, re-copy of files that changed, the seal barrier,
   manifest written last with fsync, self-verification before success. Cancellation is checked at every stage boundary and
   between files; cleanup removes the destination directory and releases the pins through the port.
4. **Share-tolerant copy engine**: per-directory listing pass, per-file copy with bounded retry on sharing violations
   (Windows), a final hash pass that defines the snapshot, files deleted mid-copy re-listed, the manifest built only from
   what the last pass hashed. Destination directory created atomically (design 4.2 step A).
5. **Manifest v2** (design 4.5): encode and decode with unknown-key preservation; `kind`, `format_epoch`,
   `binary_version`, `consistency`, `wal`; v1 manifests keep parsing with `kind = "offline"` defaulted; extend
   `verify_manifest` to v2 and keep every v1 path green. `backup.meta.json` per 4.6 with `encryption: "none"` (no
   encryption is designed or implemented).
6. **Progress** reporting structure (stage, per-repository bytes and files hashed so far, started-at, last error) that step
   3 will return from the status operation.
7. **Tests** (CI only) in the module's `tests/` folder, driving the engine through a test implementation of the port over
   plain directories: full run produces a snapshot that `verify_manifest` accepts; a file changed between copy and hash is
   re-copied; a file deleted mid-copy is absent from the manifest; cancellation at each stage leaves no destination
   directory and releases every pin; a second concurrent run is refused; manifest v1 and v2 round trips with unknown keys
   preserved; a v1 snapshot still verifies. On Windows never read a file while another handle holds a byte-range lock on it.

## Rules

* Nothing in `shamir-engine`, `shamir-db`, `shamir-server` or any engine crate in this step; no wire or DTO change; the
  offline `backup` and `restore` behave exactly as before. No new dependency.
* Layout: `shamir-sync/src/`, `sync/` and `sync/durability/` are at the 7-entry limit (design 4.1). Count them again from
  a listing; after the regroup `durability/` must hold 7 entries or fewer. At most 1000 lines per file; one primary export
  per file; `mod.rs` re-exports only; no local `use`; no inline test modules; no new `#[allow]`; no visibility widening
  beyond the new module's own API.
* Keep the change reviewable: if the feature part passes roughly 2500 lines, stop after items 1 to 5 with their tests and
  say in the report what remains.

## Acceptance (observed in CI)

fmt, `cargo clippy -p shamir-sync --all-targets -- -D warnings` and layout clean locally; the `shamir-sync` and
`shamir-server` tests green on the three OS (the server depends on the regrouped paths). The final report lists the
regroup paths apart from the feature paths, the port trait with its methods, every design point implemented and every
deviation with its reason.
