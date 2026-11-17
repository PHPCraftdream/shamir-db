# Brief: R2a step 1c — commit admission gate (primitive and leader/follower wiring only)

Design (settled, read first): `docs/dev-artifacts/design/online-backup.md`, subsection "Step 1 findings (2026-11-17),
revised into the settled capture/seal design": hazard 2, section 2 "Commit admission barrier" (primitive, entry table,
release points, waiting policy), section 4 (CI tests `commit_admission_gate::*`), section 5 "PR A", open questions Q11
and Q13. Every code claim there carries a path and line; re-verify the ones you touch (the tree has moved a little).

## Why

An overlay-only leader commit enters `commit_tx_inner`, merges its interner overlay, assigns a version and appends WAL
without touching any table token, so no table freeze stops it; the tail counter is entered only after prelock and is not
an admission gate; follower apply (`apply_replicated_batch`) is a separate entry with no gate at all. The online backup
seal needs one place that can close leader admission and await the leaders already inside. This step builds that place
and wires it in. It changes nothing observable while the gate is open.

## What (this step only)

1. **Primitive.** `CommitAdmissionGate`, `CommitAdmissionLease` (RAII, owned, `Send`), and a close owner (RAII, reopens
   on drop) in a new module `tx/commit/pipeline/admission/` of `shamir-engine` (one primary type per file, `mod.rs` is a
   manifest). One `AtomicUsize` state word: closed bit, close-owner bit, bounded active-lease count; admission is one CAS
   on an open, non-overflowed state; no `Mutex`/`RwLock` anywhere. Waiting for reopen and waiting for zero leases use a
   `Notify` armed BEFORE the atomic recheck (the pattern of `CommitTailBarrier::wait_zero`). Orderings follow the design;
   state them in the doc comments.
2. **Ownership.** One gate per repository, owned next to the commit tail barrier of the repo core, reachable from the
   commit pipeline and from the follower apply path.
3. **Leader entry.** Acquire in `commit_tx_inner`, after the wound/expiry early exits and BEFORE `repo.tx_gate()` and
   `pre_commit_prelock`, covering ordinary, overlay-only and AsyncIndex leaders. A leader that finds the gate closed waits
   for reopen, bounded by a cap (a named constant in `shamir-tunables` style if one fits, else a documented constant);
   past the cap it fails with a typed retryable error and has had no side effect. Cancellation while waiting is free.
4. **Follower entry.** Acquire at the top of `apply_replicated_batch`, before the write-domain admission of the receipt
   pipeline; a closed gate refuses at once with a typed error (design Q13).
5. **Lease lifetime.** The lease moves with the continuation: through `CommitDriver`'s suspended-future transfer on drop,
   through the AsyncIndex tail handoff (no zero-count gap), through the separately spawned panic cleanup; released only
   after publication, every mutation-capable tail and error cleanup. Cancellation after a successful CAS releases it.
6. **Close API** (used by tests in this step, by the seal in the next): close returns the RAII owner once all leases that
   were admitted before the close have dropped; a second closer gets a typed busy error; dropping the owner reopens with
   the count preserved.
7. **Tests** (CI-only, deterministic handshakes — no sleeps as correctness evidence), under
   `shamir-engine/src/tx/commit/pipeline/admission/tests/` or the crate's existing test layout for this area: close then
   an overlay-only leader is blocked/refused BEFORE it changes the base interner, takes a version or appends WAL (freezing
   tables alone is the negative control); a lease survives detached and AsyncIndex tails; follower apply respects the
   gate; cancellation at acquisition and after the CAS releases; the close-versus-acquire CAS boundary (either counted
   before the close or refused after, never uncounted); reopen after error/timeout/unwind leaks no count.

## Do not

Build the drain pause guard, the seal, the `capture_point`/`seal_point`/`replay_to` facade, admin operations, metrics or
anything in `shamir-sync`/`shamir-server`. Change no commit semantics, ordering or error text while the gate is open.
Add no dependency. Keep the open fast path to one atomic read-modify-write and no allocation (it is per commit).

## Rules that bind the change

* Layout: at most 7 entries per folder, at most 1000 lines per file (`python -I scripts/ci/check_layout.py`).
* rust-guard `structure`: no logic in `mod.rs`, no inline `#[cfg(test)]` modules (tests live in `tests/` with a manifest
  `mod.rs`), no local `use`, no new `#[allow]`, no impl outside the type's module; keep new items as narrow as the
  wiring allows (`pub(crate)` or tighter; widen only what a later step provably needs).
* Imports at the top of the file; builder-only queries in tests; multi-line indented JSON literals.
* Machine memory: run at most ONE cargo command at a time, always as
  `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`.
* Wrap-up: by 120 minutes bring the change to a compiling, formatted, clippy-clean state and stop; say in the final
  report what is missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Observable acceptance

`cargo fmt`, `cargo clippy -p shamir-engine --all-targets -- -D warnings` (plus `-p shamir-db` if a public signature
changed) and the layout gate are clean; the leader path and the follower path acquire the gate; the lease follows every
continuation listed above; the tests listed above exist and name what they prove. The final report lists the decisions
taken (primitive layout, cap value and where it lives, error types), what is not covered, and any open question.
