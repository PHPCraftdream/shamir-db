# 01 - wasm compile cache without C and without unsafe (PR #227 rewrite, DEC-3 Q5)

Owner standard: we strive for perfection; for that we are ready to do MORE work and to BREAK backward compatibility (product unreleased, no migrations).

## Task

In THIS worktree (branch `refactor/wasm-compile-cache-rust`, PR #227) rewrite the persistent WASM compile cache of `crates/shamir-wasm-host` exactly as DEC-3 Q5 below rules: wasmtime's SAFE `Config::enable_incremental_compilation(Arc<dyn CacheStore>)` with an own bounded, framed, per-key disk `CacheStore`; delete the whole-module `Module::serialize` / `unsafe Module::deserialize` cache that this branch currently contains (`crates/shamir-wasm-host/src/wasm/compile_cache.rs` and its tests). `crates/shamir-wasm-host` must end with `#![forbid(unsafe_code)]`.

Steps:
1. Read the ruling below, the current branch diff (`git diff origin/master...HEAD`), `crates/shamir-wasm-host/src/wasm/wasm_engine.rs`, `wasm_function.rs`, `lib.rs`, `Cargo.toml`, and the installed wasmtime / wasmtime-environ / cranelift-codegen sources under the cargo registry (read-only) for `CacheStore`, `enable_incremental_compilation`, `precompile_compatibility_hash`. Find where `ShamirDb::init_inner` builds the `WasmEngine` (crate `shamir-db`) and where backup/restore of `data_dir` live (`backup.rs`, `restore.rs`) and apply the wiring and the `cache/` exclusion the ruling requires.
2. Implement per the ruling: modules `crates/shamir-wasm-host/src/wasm/compile_cache/{mod.rs,disk_store.rs,entry_frame.rs,entry_index.rs,private_dir.rs,namespace.rs}` (mod.rs re-exports only; one primary export per file; imports at the top; at most 7 entries per folder; at most 1000 lines per file). Tests go to `crates/shamir-wasm-host/src/tests/wasm/compile_cache/` as a manifest `mod.rs` plus topic files, covering every case the ruling lists. Replace the old `tests/wasm/compile_cache_tests.rs` and fix `tests/wasm/mod.rs`.
3. Cargo: exactly the feature and dependency changes of the ruling (wasmtime features list, `sha2` at the version already in `Cargo.lock`, `rustix` for `geteuid` at the version already in `Cargo.lock`, drop `blake3`). After editing manifests refresh the lock WITHOUT compiling: `cargo metadata --format-version 1 > /dev/null`, then inspect `git diff Cargo.lock` and confirm that only expected packages change (zstd / zstd-sys / wasmtime-internal-cache and friends disappear; `blake3`/`constant_time_eq`/`arrayref`/`arrayvec` may disappear only if nothing else uses them). Report the lock diff summary.
4. Do not bump any project version. Write no private machine paths (drive letters, user directories) anywhere. Comments: concise. No `unsafe` anywhere in the crate.
5. Local gates (the only allowed commands; run until clean, one cargo process at a time, `CARGO_BUILD_JOBS=4`): `cargo check -p shamir-wasm-host --all-targets`, `cargo clippy -p shamir-wasm-host --all-targets -- -D warnings`, `cargo fmt -p shamir-wasm-host`, the same three for `shamir-db` (and any other crate you touched), and `python -I scripts/ci/check_layout.py`. Tests and benches are banned locally - the PR's CI runs them.
6. Final message (mandatory): file list with line counts; each ruling bullet -> where implemented; the exit codes of the gates; the lock diff summary; anything in the ruling you could not implement exactly, with the reason; the unresolved questions about wasmtime 48.0.7 source parity (the orchestrator re-checks them in CI).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.

## Ruling to implement (verbatim from DEC-3, Q5)

## Q5 - Compile cache with no C and no `unsafe` (#227)

**RULING:** Rewrite #227 around wasmtime's safe `Config::enable_incremental_compilation` with a bounded, framed, per-key disk `CacheStore` under `<data_dir>/cache/wasm/`, on Unix only at first. Delete the whole-module serialize/deserialize cache.

### What the key covers, and the trust boundary
- Soundness across settings: Cranelift's key (stencil + ISA + flags + ISA flags) is upstream's reuse contract. Fuel and epoch instrumentation and memory bounds checks are in the IR. The Cranelift version is not in the key but is rejected when the value is deserialized. The design adds an outer namespace over `Engine::precompile_compatibility_hash()` (tunables, features, flags, triple, wasmtime version), so different engine settings (fuel, epoch, pooling, `SHAMIR_WASM_NO_POOL`) never share entries.
- Trust is moved, not removed: a tampered entry is executable code. The safe API removes our `unsafe` call site, not the attack. A digest detects corruption only. Therefore: private directory, never replicated, never in a backup or restore, never sourced from an environment variable.

### Exact specification

**`Cargo.toml` (`shamir-wasm-host`):**
- wasmtime features: `["cranelift","runtime","std","wat","async","pooling-allocator","incremental-cache"]`. `cache` is removed; the feature adds no new crates (verified).
- `sha2` at the version already in the lock (0.10.x).
- `[target.'cfg(unix)'.dependencies] rustix = { version = "<lock version used by wasmtime>", default-features = false, features = ["std","process"] }` for `geteuid`.
- Drop `blake3`.
- `src/lib.rs` gets `#![forbid(unsafe_code)]`.

**Modules (`src/wasm/compile_cache/`):** `mod.rs` (re-exports), `disk_store.rs`, `entry_frame.rs`, `entry_index.rs`, `private_dir.rs`, `namespace.rs`.

**Wiring:**
- `WasmEngine::new()` means no persistent cache.
- `WasmEngine::with_compile_cache(dir)`: (1) builds `Arc<DiskCacheStore>` unbound (a `OnceLock`); (2) calls `config.enable_incremental_compilation(store.clone())` before `Engine::new`; (3) after the final engine exists (including the pooling fallback), calls `store.bind(&engine)`, which validates the directory, computes the namespace and scans the index. An unbound or failed store returns `get -> None` and `insert -> false`.
- `ShamirDb::init_inner` passes `data_root.join("cache").join("wasm")` when `data_root` is `Some`. For an in-memory DB, or on Windows, it uses `new()`.
- `WasmEngine::compile` stays a plain `Module::from_binary`.

**Namespace:** `ns = SHA-256("shamir-wasm-icache\0" || FORMAT_VERSION u32 LE || CARGO_PKG_VERSION || engine.precompile_compatibility_hash() streamed through a SHA-256 std::Hasher)`. The directory is `v1-<first 32 hex of ns>`. At bind, every other `v*-*` namespace directory is deleted.

**On-disk layout:**
```
<data_dir>/cache/wasm/v1-<ns>/tmp/
<data_dir>/cache/wasm/v1-<ns>/<00..ff>/<64-hex key>.e
```

**Frame:**

| Offset | Bytes | Content |
|---|---|---|
| 0 | 8 | magic `SHMWIC\0\1` |
| 8 | 32 | ns |
| 40 | 32 | key |
| 72 | 8 | payload_len, u64 LE |
| 80 | 32 | SHA-256(bytes[0..80] || payload) |
| 112 | payload_len | payload |

**Limits:** payload <= 4 MiB (larger values: `insert -> false`); total <= 256 MiB; entries <= 65,536; when either cap is exceeded, trim to 75% by the oldest logical tick, single trimmer guarded by an `AtomicBool`.

**Index:** `scc::HashMap<[u8;32], EntryMeta{len, tick: AtomicU64}, THasher>`, plus `AtomicU64` total bytes and `AtomicUsize` count (no `scc::len`). Built at bind from directory metadata only.

**`get`:** key length != 32, or not in the index -> `None` with no I/O; otherwise check `metadata.len() == 112 + len <= cap` before allocating, read exactly, verify magic/ns/key/len/digest; on any mismatch remove the file and index entry, increment `rejected`, return `None`; a hit bumps the tick.

**`insert`:** key already present -> `true`; write `tmp/<pid>-<seq>-<keyhex>` with `create_new` and mode 0600, no fsync; rename to the final path; update the index; trim if needed; any error -> `false`. Startup removes everything in `tmp/`. Torn or absent files are misses. Cache failure never fails module validity.

**Directory trust (Unix, std plus `rustix::process::geteuid`):** canonicalize `data_dir`; `cache` and `cache/wasm` are created with mode 0700; `cache` and `cache/wasm`: no symlinks (`symlink_metadata`), `uid == euid`, `mode & 0o077 == 0`; `data_dir`: `uid == euid`, `mode & 0o022 == 0`; every ancestor up to `/`: `uid in {0, euid}` and (`mode & 0o022 == 0` or sticky); any violation -> disk tier disabled, logged at warn.

**Windows:** disk tier disabled in this PR.

**Backup and restore:** `backup.rs` skips the top-level `cache/` subtree. `restore.rs` rejects a snapshot manifest that lists any `cache/` path.

**Delete from #227:** `Module::serialize`, `unsafe { Module::deserialize }`, and the key derived from the wasm bytes; `CompileCache::module` and `Outcome`; `default_dir`, `SHAMIR_WASM_CACHE_DIR`, `SHAMIR_WASM_NO_CACHE`, and the LOCALAPPDATA/XDG/HOME path logic; the blake3 framing and the `blake3` dependency; trim-at-open-only and the mode-bits-only `create_private_dir`; the routing of `WasmEngine::compile` through the cache; the current `compile_cache_tests.rs`.

**Keep from #227:** removal of the `cache` feature and of the `wasmtime::Cache::new(CacheConfig::new())` block; the resulting Cargo.lock removals (zstd, zstd-sys, wasmtime-internal-cache, ...); `from_wat` compiling the sanitized `wasm_bytes`; the single `WasmEngine::compile` entry point.

**Tests (`src/tests/wasm/compile_cache/`, CI):** a second engine on the same directory gets hits (store counters); a flipped payload byte -> miss and the file removed; a frame with the wrong key -> miss; an oversized file is rejected without being read; changing `consume_fuel` gives a new namespace and removes the old one; trimming respects the caps; concurrent same-key inserts; group-writable or symlinked directory -> disabled; cold vs warm runs give identical results for fuel-exhaustion, epoch-interrupt and memory out-of-bounds traps; backup excludes and restore rejects `cache/`.

**Before the PR goes ready:** re-read the 48.0.7 / 0.135.7 sources that `cargo check` downloads (`config.rs`, `compiler.rs` `compile_maybe_cached`, `incremental_cache.rs`, `ir/function.rs` VersionMarker) and confirm they match the analysis above.

### Rejected alternatives
An audited micro-crate around `deserialize` (still our `unsafe`, same executable-code trust); a database-stored artifact (still needs `deserialize`, puts machine code inside the replication and backup trust domain); Pulley (different execution model, not a cache answer); no cache (kept only as the fallback).

**"No C" is not achieved by this PR:** `wasmtime/build.rs` compiles `helpers.c`, and `aws-lc-sys`, `ring`, `libmimalloc-sys` are in the lock. That is an owner decision.

### Reviewer disagreements
- Location: all three chose an OS-level private service directory; @oh chose `data_dir` (per instance, tied to the node's lifecycle, backup exclusion added).
- Windows: reviewers wanted ACL validation; @oh disables the disk tier until Q5-c (the only ACL code in the repo is `unsafe` FFI).
- Default: ops and adversary wanted it optional until measured; @oh chose on by default for disk-backed databases on Unix, with Q5-d deciding whether it stays.
- Extra in-memory tier (adversary): rejected; the registry already holds compiled `InstancePre`.
- Hash: sha2 instead of blake3 (already in the graph).

### Implementation order
1. Q5-a, the #227 rewrite, after PR-SC (48.0.7) - CI-only.
2. Q5-b: wrap `WasmFunction::from_binary` at the async call sites (`core.rs` boot loop, `function_management.rs`, `validator_management.rs`) in `tokio::task::spawn_blocking` - CI-only.
3. Q5-c: extract the owner-only DACL code from `shamir-transport-ipc/src/windows.rs` into one shared crate with a safe `create_private_dir` / `verify_private_dir`, plus a reparse-point check through std `file_attributes() & 0x400`. Enable the Windows disk tier - CI-only.
4. Q5-d: bench cells `compile_cold` / `compile_warm` / `boot_k_functions` in `crates/shamir-engine/benches/wasm_invoke.rs` (`bench_scale_tool`), run through perf-gate `workflow_dispatch` - CI-only.

### What would change my mind
Q5-d warm/cold > 0.8 on the representative module set: remove the disk tier and ship no persistent cache (never bring back `deserialize`). An upstream advisory against incremental-cache soundness, or a 48.0.7 behaviour change: disable it.

---

