# Brief: R2a step 1 — consistency point, WAL truncation pin and replay facade in the engine (release-1 R2a, implementation)

Design: `docs/dev-artifacts/design/online-backup.md` — read it in full, the "Review" section included. This is step 1 of
its implementation plan (section 7). Sections 1.3 (storage facts), 4.2 (protocol: steps A, B, E, G), 4.3 (checkpoints,
retention, compaction), 4.4 (concurrency) and 6 (tests) are binding for this step. Step 2 (the copy engine in
`shamir-sync`) is built in parallel by another change; it defines a small port trait that step 3 will connect to this
facade. Do not depend on `shamir-sync` and do not implement its trait here.

## Why

Release gate: "a backup is taken without stopping the server, restores on a clean machine, and a test under write load
proves it". The copy engine needs the engine to (a) pin WAL truncation so the segments a copy needs survive, (b) capture a
per-repository flushed position and the system-store version, (c) name one consistency point `V*` for the whole copy, and
(d) replay a copied repository's WAL up to `V*` on restore.

## What

1. **Facade** (design section 7 step 1, `db_instance` layer of `shamir-engine`): `capture_point`, `seal_point` and
   `replay_to` with exactly the data design 4.2 names (`wal_position` per repository, the system-store version, `V*`),
   composing `flush_all_history`, the new truncation pin and `max_committed`. Each method's doc names the protocol step it
   serves.
2. **Truncation pin** (design 4.3): a pin that the drainer's truncate path honours, released by an RAII guard on every exit
   path (success, error, cancellation, drop). Several pins may coexist; truncation never passes the lowest live pin. The
   pin is the only tx-layer change; keep it minimal and lock-free on the truncate path (an atomic minimum or the existing
   retention-gate pattern — say which and why).
3. **Locations moved.** The engine was split since the design was written: `shamir-engine-repo` and
   `shamir-engine-table` now hold the repo and table layers. Re-locate every item the design cites
   (`durability_access.rs:263` `flush_all_history`, `segment_set.rs:730` `max_committed`, `retention_gate.rs:89`,
   `system_store/store.rs:328-333`, `truncate.rs:76-104`) from a fresh search and report the current path of each.
4. **Visibility.** Widen only what the facade needs, item by item, each declared to rust-guard (`.rust-guard` structure
   gate: visibility widening vs the merge base needs a declaration; read `tools/rust-guard/README.md` "Delta file" and the
   current `.rust-guard/stages.toml` to see whether a campaign delta applies, otherwise the per-item allow mechanism the
   README describes). Prefer a facade method over widening an internal item.
5. **Tests** (CI only) with the existing drain/retention seams: a pin holds truncation back and its release lets it
   proceed; two pins — the lower one governs; the guard releases on panic/drop; `capture_point` then writes then
   `seal_point` yields a `V*` that covers exactly the committed writes up to the seal; `replay_to(V*)` on a copied
   directory reproduces the state at `V*` and nothing after it.

## Rules

* Crates: `shamir-engine` (and, only where an item now lives there, `shamir-engine-repo` / `shamir-engine-table`). No
  `shamir-sync`, `shamir-server`, `shamir-db` or wire/DTO change; no change to the offline `backup`/`restore`. No new
  dependency.
* Layering: `.rust-guard/layers.toml` — no new backward edges; the facade sits in `db_instance`, the pin in the tx layer;
  calls flow downward only.
* At most 7 entries per folder and 1000 lines per file; one primary export per file; `mod.rs` re-exports only; no local
  `use`; no inline test modules; no new `#[allow]` without a declaration.
* Code ideology: no `Mutex`/`RwLock` on the truncate or commit path.
* Keep the change reviewable: if it passes roughly 2000 lines, stop after items 1–4 with their tests and report.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Acceptance (observed in CI)

fmt, `cargo clippy -p shamir-engine --all-targets -- -D warnings` (plus each other touched crate) and layout clean locally;
the touched crates' tests green in CI on the three OS. The final report lists every file created and changed, the current
path of every item re-located in item 3, every widened item with its declaration, the pin primitive chosen, and every
deviation from the design with its reason.
