# DEC-3 Q5-c — shared safe private-directory crate and the Windows wasm disk tier

## Context

Owner-approved ruling DEC-3 Q5 (`#227` rewritten in Q5-a/Q5-b, both merged): the
wasm compile cache is a bounded on-disk `CacheStore` under `<data_dir>/cache/wasm/`,
enabled on Unix only. A tampered cache entry is executable code, so the directory
must be owner-private, never a symlink/reparse point, and verified at every open.
On Windows the disk tier is disabled today; the only ACL code in the repo is the
`unsafe` FFI in `crates/shamir-transport-ipc/src/windows.rs` (owner-only DACL for
named pipes). Q5-c extracts that code into ONE shared crate with a SAFE public API
(`create_private_dir` / `verify_private_dir`) and turns the Windows disk tier on.
This task is Q5-c ONLY: not Q5-b (merged), not the benches (Q5-d).

Read first: `crates/shamir-transport-ipc/{Cargo.toml,src/lib.rs,src/windows.rs,src/tests/}`,
`crates/shamir-wasm-host/{Cargo.toml,src/lib.rs,src/wasm/compile_cache/*,src/tests/wasm/compile_cache/*}`,
`crates/shamir-db/src/shamir_db/shamir_db/core.rs` (the `#[cfg(unix)]` / `#[cfg(not(unix))]`
engine construction around line 266), root `Cargo.toml` (members are `crates/*`),
`CLAUDE.md` / `AGENTS.md` workspace section (crate list and count).

## Scope

1. **New workspace crate `crates/shamir-private-dir`** (`publish = false`, edition 2021,
   `[lints] workspace = true`, `[lib] doctest = false`, license/repository fields like
   the other crates, version like the other pre-release crates). It is the ONLY place
   for the `unsafe` Windows FFI of this feature (`#![forbid(unsafe_code)]` stays in
   `shamir-wasm-host`). The module with `unsafe` is `#[cfg(windows)]`; every `unsafe`
   block carries a `// SAFETY:` comment. Layout: `src/lib.rs` (re-exports only),
   `src/unix.rs`, `src/windows.rs` (+ child files if over 1000 lines or over 7
   entries), `src/tests/` per the repo test layout (the tests manifest `mod.rs` is
   pure `mod`). Dependencies: only crates already in `Cargo.lock` at their locked
   version: `windows-sys = "0.60"` with the feature set `shamir-transport-ipc` uses
   today (add `Win32_Storage_FileSystem` only if the code needs it), and `rustix`
   (same version/features as `shamir-wasm-host` uses) for `geteuid` on Unix. No new
   third-party crate, no version bump of anything else.
2. **Public API (safe, no `unsafe` in signatures):**
   - `create_private_dir(path: &Path) -> io::Result<()>`: creates the directory (one
     level, the parent must exist; an already existing directory is accepted only if
     `verify_private_dir` passes) that only the current OS account can access. Unix:
     mode `0700`. Windows: created atomically with an explicit security descriptor
     `D:P(A;OICI;FA;;;<current-user-SID>)` (protected DACL, one ACE: inheritable full
     access for the current process token's user SID, same SID derivation as the
     named-pipe code), never "create then fix up".
   - `verify_private_dir(path: &Path) -> io::Result<()>`: Unix: `symlink_metadata`
     says real directory (no symlink), `uid == geteuid`, `mode & 0o077 == 0`.
     Windows: the path is a real directory and NOT a reparse point (std
     `MetadataExt::file_attributes() & 0x400 == 0`, read without following links),
     the owner is the current user SID, and the DACL is protected and contains
     exactly one ACE granting the current user SID (compare the SDDL string of
     owner+DACL read from the directory with the expected one, or walk the ACL;
     choose the simplest correct approach and document it). Any violation is
     `io::ErrorKind::PermissionDenied` with a message naming what failed.
   - A small safe type for the named-pipe consumer, `OwnerOnlySecurityDescriptor`
     (Windows only) with `for_current_user() -> io::Result<Self>` and an accessor
     returning the raw `SECURITY_ATTRIBUTES` pointer (returning a raw pointer from a
     safe fn is fine; dereferencing stays the consumer's `unsafe`), `Send + Sync`
     exactly as today.
3. **`shamir-transport-ipc`**: delete the SDDL/SID/`LocalFree` code from `windows.rs`,
   depend on the new crate and use `OwnerOnlySecurityDescriptor` (the pipe behaves
   identically: same SDDL, same security attributes, `reject_remote_clients(true)`,
   `first_pipe_instance` handling). Remove the now unused `windows-sys` features from
   its `Cargo.toml` (drop the dependency if nothing remains). The crate keeps its
   single remaining `unsafe` call (`create_with_security_attributes_raw`) with its
   SAFETY comment. Its public API does not change.
4. **`shamir-wasm-host`**: `src/wasm/compile_cache/private_dir.rs` keeps its Unix
   `prepare` unchanged (std + rustix, ancestor checks) and gets a Windows
   `prepare(data_dir)` built on the new crate: canonicalize `data_dir`; verify
   `data_dir` itself is a real directory and not a reparse point; every component
   below `data_dir` that the code creates (`cache`, `cache/wasm`) goes through
   `create_private_dir` when missing and `verify_private_dir` when present; any
   violation disables the disk tier through the existing "disabled, logged at warn"
   path, never panics and never fails engine construction. `disk_store.rs`: entry
   files on Windows use `create_new` (no mode bits); everything else unchanged.
   Remove the `cfg(not(unix))` stub and make `DiskCacheStore` and its tests compile on
   both platforms: delete `#[cfg(unix)]` gates that only existed because of the
   disabled Windows tier, keep gates that guard truly Unix-only APIs (`rustix`, mode
   bits, symlink creation in tests). Add `shamir-private-dir` as a dependency
   (`cfg(windows)` only if that keeps Unix free of an unused dependency).
5. **`shamir-db`** `core.rs`: remove the `#[cfg(unix)]` / `#[cfg(not(unix))]` split so
   every platform passes the data root to `WasmEngine::with_compile_cache`; update
   the comment (the disk tier is private-directory-verified on Unix and Windows).
6. **Tests (run by CI; you cannot run them, write them carefully):** in the new
   crate's `src/tests/`: create+verify roundtrip on a temp dir; `verify` rejects a
   directory with a broader DACL / mode (Windows: add a second ACE for Everyone with
   `std::process::Command` calling `icacls`, or an SDDL helper in the test; choose
   one, keep it robust); `verify` rejects a directory junction/symlink (Windows:
   `mklink /J` through `std::process::Command("cmd")`, skip with a clear message if
   the privilege is missing) and a regular file; `create_private_dir` is idempotent
   on a verified directory. In `shamir-wasm-host`: the `compile_cache` tests that
   were `#[cfg(unix)]` run on Windows too where meaningful (second engine on the same
   directory gets hits, corrupted entry rejected, cold vs warm trap parity, trim);
   Unix-only directory-trust tests (group-writable, symlinked) stay Unix-only; add the
   Windows counterparts (broader DACL gives a disabled disk tier, junction as `cache`
   gives a disabled disk tier). Replace the obsolete `windows_degrade_tests.rs`.
   Wire everything through pure-`mod` manifests; queries (if any) via the query builder.
7. **Docs and registries**: crate lists/counts in `CLAUDE.md`, `AGENTS.md` and any
   workspace doc that enumerates crates; mentions of "disk tier is Unix only" in
   `docs/` and in the `compile_cache/mod.rs` header; `Cargo.lock` for the new member
   (dependency rules below).

## Dependency rules (hard)

- Every dependency of the new crate is already in `Cargo.lock`; use the SAME version
  string the other crates use. Do not run `cargo update`. After adding the member,
  `cargo metadata --locked --format-version 1` must succeed (if it complains that
  `Cargo.lock` needs the new package entry, add exactly the new `[[package]]` block
  and the dependency edges by hand, alphabetical, no version change, and re-run until
  it is happy). Report the exact lock diff.
- No `[patch]`, no feature additions to existing dependencies beyond the
  `windows-sys` features named above.

## Do not

- Do not touch `crates/shamir-engine/src/repo/` or `crates/shamir-engine/src/tx/`
  (layout stages D2/E1 run there), `.rust-guard/**`, `.github/**`, the project
  version.
- Do not weaken the Unix checks, do not change the cache format, namespace, limits or
  the `WasmEngine` API.
- Layout limits: at most 7 entries per folder, at most 1000 lines per file; no
  `mod.rs` with logic; imports at the top of the file; no `#[allow]` without a
  one-line reason.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index
  (including `git add` / `git commit`). Only edit files; the orchestrator commits.
- Compile / build / test policy: see the policy text at the top of this run
  (verbatim). Allowed locally and REQUIRED until clean (this machine is Windows, so
  the Windows code is compiled; the Unix code is CI-only, so re-read it carefully):
  `cargo check -p shamir-private-dir --all-targets`,
  `cargo check -p shamir-transport-ipc --all-targets`,
  `cargo check -p shamir-wasm-host --all-targets`,
  `cargo check -p shamir-db --all-targets`,
  `cargo check -p shamir-server --all-targets`,
  `cargo clippy -p <crate> --all-targets -- -D warnings` for each of the four touched
  crates, `cargo fmt -p <crate>` for each touched crate and then `-- --check`,
  `python -I scripts/ci/check_layout.py`, `cargo metadata --locked`. One cargo
  process at a time, `CARGO_BUILD_JOBS=4`. No `cargo test`, no `cargo build`.
- NEVER stop processes by image name or pattern (`Stop-Process -Name`,
  `taskkill /IM`, `pkill`); the machine runs other sessions' cargo.
- Never use the shell builtin `xargs`. Capture exit codes without pipes. Logs under
  `.scratch/`. No machine-specific absolute paths in any file.
- Before finishing, one verifier child agent (the whole policy pasted verbatim) reads
  every changed file in full and checks: every `unsafe` block has a SAFETY comment and
  sits in `#[cfg(windows)]` code of the new crate (plus the single pipe call in
  transport-ipc); the named-pipe SDDL and attributes behave identically; every
  `#[cfg(unix)]` removed in wasm-host/db had no remaining Unix-only API under it; the
  Unix `prepare` is unchanged; the Windows verification really checks owner,
  protected DACL, single ACE and the reparse-point bit; the tests are wired into
  pure-`mod` manifests. Start at most one child at a time.

## Final message

1. Per scope item 1-7: done / not done with file:line.
2. The exact `Cargo.lock` diff and the result of `cargo metadata --locked`.
3. How `verify_private_dir` decides "the DACL is exactly the owner" on Windows and why
   that is robust.
4. Real exit codes of every gate above; `ls -A` of `crates/shamir-private-dir` and its
   `src/`.
5. Anything you could not verify (Unix paths; the test runs; junction/icacls privilege
   assumptions).
