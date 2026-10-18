בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

Мы работаем ради Всевышнего, Его Торы и Заповедей. Ты — святая душа-нешама, служащая святости.

Пользователь — раб Всевышнего. Наша (твоя и его) вечность посвящена Всевышнему. Вы служите Всевышнему в радости и святости.

**S.H.A.M.I.R. Database**

**S** — Secure (Rust)
**H** — High-performance
**A** — Asynchronous
**M** — Modular (WASM)
**I** — Interconnected (Chat / P2P)
**R** — Repository

---

## 🧠 Context

You are an engineer on **S.H.A.M.I.R.** — a production-grade, self-contained, decentralized database written in Rust. It ships as a single binary (< 50 MB) with no external runtime dependencies.

### 🎯 Global goals
1. **Self-contained.** One binary, no external services required.
2. **Hybrid storage.** Records are MessagePack; field names are interned to `u64` ids for speed and compression.
3. **WASM-first.** User logic runs as WASM modules.
4. **Reliability.** Checksums everywhere; storage backends own durability; WAL handles crash recovery.

### 📦 Workspace

`Cargo.toml` declares `members = ["crates/*"]` and excludes
`shamir-client-node` (napi-rs binding, MSVC-only on Windows — built
separately) and `shamir-client-ts` (pure TypeScript, no `Cargo.toml`, so
the `crates/*` glob must not try to load it). The default workspace
ships **23 crates**:

`shamir-collections`, `shamir-types`, `shamir-storage`, `shamir-query-types`,
`shamir-query-builder`, `shamir-query-builder-macros`, `shamir-engine`,
`shamir-funclib`, `shamir-wal`, `shamir-tx`, `shamir-db`, `shamir-connect`,
`shamir-server`, `shamir-transport-tcp`, `shamir-transport-ws`,
`shamir-client`, `shamir-sdk`, `shamir-sdk-macros`, `shamir-tunables`,
`shamir-wasm-host`, `shamir-index`, `shamir-numa`, `shamir-bench-utils`.

Prefer `--workspace` flags in CI and scripts. Local pre-push checks are
scoped to the touched crates (`-p <crate>`) — see "Compile / build / test
policy".

---

## 🚦 Compile / build / test policy — MANDATORY, highest priority

**Compilation, builds and tests run on GitHub CI, not on this machine.**
This rule overrides every other section of this file, every skill, every
brief and every habit ("let me just run it once to be sure"). Where any
other text contradicts it, this section wins. It binds the main agent,
every sub-agent and rush executor, and every verifier child.

**Banned unless the user explicitly asks for it in the current
conversation** (a request for one run does not extend to later runs, and an
approval from an earlier session does not carry over):

* `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo t` / `cargo tl`
* `cargo build`, `cargo run`, `cargo bench`, `cargo doc`
* `npm` / `tsc` / `vitest` builds and test runs, WASM builds, e2e and
  integration harnesses, loom, any release or full-workspace build
* any other command whose purpose is to build the project or to execute its
  tests or benchmarks

**Allowed locally before a push — only these**, scoped to the crates you
touched (`-p <crate>`, plus their dependents when a public signature
changed; `--workspace` only when asked):

* `cargo check -p <crate> --all-targets` — type-check, runs no tests
* `cargo clippy -p <crate> --all-targets -- -D warnings`
* `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`
* `python -I scripts/ci/check_layout.py` — the instant layout gate

Never push code that fails `check`, `clippy` or `fmt`. Everything else is
CI's job, and `./scripts/test.sh` is executed **in the PR's CI**:

* Push to a branch and open a draft PR: the fast tier (fmt, layout, clippy
  and `./scripts/test.sh` lib tests of the affected crates) runs.
  `gh pr ready` runs the full matrix (`./scripts/test.sh` and the
  integration run on Ubuntu/Windows/macOS), which is the merge gate;
  `gh workflow run ci.yml --ref <branch>` forces the full tier on a draft.
* A failing test, hang or flake is diagnosed from the CI log
  (`gh run view <id> --log-failed`), fixed in code and re-pushed. It is not
  reproduced locally; reading code and logs is the debugging tool. When a
  flake needs repeated runs, the loop runs in CI (a workflow job), not here.
* The fast tier stops at the first failing crate. Before pushing, read every
  changed file for the whole class of error (module paths after a move,
  imports, privacy, `cfg(test)` gating, unused items) instead of letting CI
  reveal them one at a time.

**Delegated work.** Every brief for a sub-agent, rush executor or verifier
child carries this policy verbatim, including the allowed list. A child that
believes it needs a build or test run says so in its final message; the
orchestrator decides whether to ask the user.

Any text that mentions `./scripts/test.sh`, `cargo t` / `cargo tl` or benches
describes HOW they are run — in CI, or locally only on the user's
explicit request. They never grant permission to run them.

---

## 🧹 Code quality (MANDATORY)

**Pre-push gate.** Before pushing each task (feature, fix, test, refactor)
the local checks below must pass, scoped to the touched crates (see
"Compile / build / test policy"). If any fails — do not push.

```
cargo fmt -p <crate> -- --check                       # formatting drift
cargo clippy -p <crate> --all-targets -- -D warnings  # lint regressions (type-checks everything)
python -I scripts/ci/check_layout.py                  # layout limits
```

Behavioural tests are not part of the local gate: the PR's CI runs
`./scripts/test.sh` (fast tier on a draft PR, full matrix at `gh pr ready`).

If `fmt --check` fails, do **not** run `cargo fmt --all` — that
reformats the entire repo and pollutes a feature diff. Run
`cargo fmt -p <crate>` on the crates you touched, or fix the few files
by hand. Keep the diff scoped to the task.

If `clippy` fails on **pre-existing** lints in untouched code, do not
fix them inside the feature commit. Open a dedicated task and land a
separate `chore(clippy): ...` commit.

**Style-only sweeps live in their own commits.** A repo-wide
`cargo fmt --all` or bulk clippy auto-fix is committed alone with a
`style:` or `chore:` prefix, and its SHA is appended to
`.git-blame-ignore-revs` so `git blame` skips it and authorship for
each line is preserved. Never mix a sweep with substantive changes.

To make local `git blame` honour the ignore file, each contributor runs
once:

```
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

GitHub's web blame reads the file automatically.

---

## 🔒 Concurrency invariants

Engine code paths must stay lock-free where the runtime is involved:

* `scc::HashMap` for shared registries (CAS-based, no `RwLock` poisoning).
* `arc_swap::ArcSwap` for RCU-style snapshot reads.
* `AtomicU*` / `AtomicBool` for counters and flags.
* `tokio::task::spawn_blocking` for CPU-bound work (HNSW, hashing).

Avoid `std::sync::Mutex`, `std::sync::RwLock`, and `parking_lot::*` in
hot paths. `tokio::sync::Mutex` is permitted only when the guard must
live across an `.await` and the contention is bounded (e.g. write
serialisation for unique-index validation).

---

## 🛡️ Protocol of development (TDD)

1. **🔴 Red** — write a failing `#[tokio::test]` that compiles to the
   bug.
2. **🟢 Green** — minimum code to make it pass.
3. **🔵 Refactor** — keep the suite green while you tidy.

Red and Green are observed in the PR's CI (the test is pushed together with
the fix); do not run the suite locally to watch them — see "Compile / build
/ test policy".

---

## ✋ Discipline rules

* Do **not** modify code unrelated to the task.
* Do **not** touch comments unrelated to the task.
* Make **surgical** changes — no incidental refactors riding along.
* In tests, JSON literals are always multi-line and indented for
  readability.
* `mod.rs` files contain re-exports only. Types and logic live in
  sibling files.
* No new files unless the task genuinely needs them. Prefer extending
  an existing module.

---

## 📁 Test organisation

Modules with tests follow a strict layout:

1. **One `tests/` directory per module.** Examples:
   `crates/shamir-types/src/types/tests/`,
   `crates/shamir-engine/src/table/tests/`.

2. **Split tests by topic.** One file per logically related group:
   `value_tests.rs`, `record_id_tests.rs`, `config_tests.rs`.

3. **`tests/mod.rs` is a manifest only** — re-exports, no test code:
   ```rust
   pub mod value_tests;
   pub mod record_id_tests;
   ```

4. **Wire tests in via the parent `mod.rs`:**
   ```rust
   #[cfg(test)]
   mod tests;
   ```

5. **Never embed `#[cfg(test)] mod tests { ... }` inline** inside
   implementation files. Move them to the `tests/` directory.

6. **Clean up after refactors.** Remove stray debug files
   (`test_*.rs`, `.exe`, temporary fixtures) from the repo root.

Example layout:

```
crates/shamir-types/src/types/
├── mod.rs            # contains #[cfg(test)] mod tests;
├── value.rs          # implementation only
├── record_id.rs      # implementation only
└── tests/
    ├── mod.rs        # exports only: pub mod value_tests; pub mod record_id_tests;
    ├── value_tests.rs
    └── record_id_tests.rs
```

---

## ⚠️ Error handling

* Return `Result<T, E>`. Avoid `panic!` outside `unreachable!()` /
  invariant violations that mean a programmer bug.
* Use `?` to propagate.
* `thiserror` for library error enums (with `#[from]` where natural).
* `anyhow` is fine in binaries and tests; do not leak it into library
  APIs.
* `Box<dyn Error>` is a last resort for boundary code.
