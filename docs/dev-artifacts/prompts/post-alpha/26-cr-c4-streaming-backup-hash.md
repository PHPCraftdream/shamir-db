# Brief: CR-C4 — backup/restore: streaming SHA-256 + copy (#779)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — verified against the current tree 2026-07-23 (post CR-B7)

`crates/shamir-server/src/backup.rs` reads entire files into a `Vec<u8>`
purely to hash them, in TWO places:

1. `collect_manifest_entries` (~line 307): `let contents = fs::read(&path)?;`
   then `sha256(&contents)` — called once per file while building a fresh
   backup's manifest.
2. `verify_manifest` (~line 365): `let contents = fs::read(&file_path)...;`
   then `hex::encode(sha256(&contents))` — called once per manifest entry
   while re-verifying a snapshot (this is ALSO on the restore path, since
   `restore.rs` calls `backup::verify_manifest` before trusting a
   snapshot).

On a large fjall SST/journal file (this codebase's data files are exactly
this shape — see `backup.rs`'s own module doc), each of these fully
materializes the file's bytes in RAM just to compute a checksum — a
needless memory spike during backup/restore proportional to the LARGEST
single file in `data_dir`, not the amount of work actually being done
(hashing needs to see the bytes once, not hold them all at once).

`copy_dir_recursive` (~line 480) uses `fs::copy(&path, &target)` for the
actual file copy — this does NOT materialize the whole file in a Rust-level
buffer (the OS/stdlib handles this reasonably, often via a kernel-level
copy path) — it is not itself the RAM problem. **Do not rewrite the copy
mechanism unless you're doing the optional single-pass fusion below** —
the mandatory fix is narrowly about the two `fs::read()` whole-file hash
sites.

## Fix (mandatory) — stream both hash sites through a fixed-size buffer

`crates/shamir-connect/src/common/crypto.rs`'s `sha256(data: &[u8]) ->
[u8; 32]` (~line 84) is a one-shot whole-buffer helper backed by
`sha2::{Digest, Sha256}` (already imported there, ~line 21) — it cannot be
reused for a streaming hash as-is (it needs the whole slice up front).
Add `sha2 = "0.10"` as a DIRECT dependency of `shamir-server`'s
`Cargo.toml` (matching the version already pinned in
`shamir-connect/Cargo.toml`, ~line 27 there — same version, don't
introduce a second pin) so `backup.rs` can drive an incremental
`Sha256::new()` / `.update(chunk)` / `.finalize()` hasher directly.

For BOTH `collect_manifest_entries` and `verify_manifest`'s per-file hash
step: replace `fs::read(&path)` + `sha256(&contents)` with a loop that
opens the file (`std::fs::File::open`), reads into a FIXED-SIZE buffer
(e.g. 1 MiB — `let mut buf = [0u8; 1024 * 1024];` or a `Vec` sized once
and reused across the loop's iterations, not reallocated per chunk),
feeds each read chunk into a running `Sha256` hasher via `.update(&buf[..n])`,
and accumulates the total byte count as it goes (replacing
`contents.len()` for the `size_bytes` field) — finalize the hasher once
EOF is reached (`read` returns `0`). Use `std::io::Read::read` in a loop
(`file.read(&mut buf)?` until `0`), not `read_to_end` (which would
recreate the whole-file-in-RAM problem this task fixes).

**Byte-identical output is the hard invariant**: the resulting SHA-256
digest and byte count must be EXACTLY the same as the current whole-file
`fs::read` + `sha256(&contents)` computation would produce, for any given
file's contents — this task changes HOW the hash is computed, not the
hash algorithm, the manifest format, or any wire-visible behavior. Verify
this yourself with a test before considering the task done (see Tests
below).

## Fix (optional, only if clean) — single-pass copy+hash fusion for backup's OWN file writes

The task's broader intent (per the review) is also to avoid reading each
backed-up file's bytes via TWO separate passes during a single `backup()`
call: `copy_dir_recursive` (~line 480, called from `backup()` to produce
the snapshot copy) reads+writes each file once, then `write_manifest` →
`collect_manifest_entries` (~lines 266, 291) reads EVERY file a SECOND
time just to hash it. If you can cleanly restructure `backup()`'s flow so
a single recursive walk reads each source file ONCE, writes it to the
destination via the same buffered loop, AND feeds the same chunks into a
running hasher (producing the manifest entry directly from that one
pass) — do it, since it's a genuine 2x-fewer-file-reads win during
backup. This is OPTIONAL: `copy_dir_recursive` is also called from
`restore.rs` (marked `pub(crate)` specifically for that reuse, per its
doc comment ~line 478) for a plain copy with NO hashing needed at that
point (restore's own manifest verification is a SEPARATE, later step
against the ALREADY-copied files) — so if fusing copy+hash requires
restore.rs's call site to thread through hashing output it doesn't need,
or otherwise complicates that shared function's signature awkwardly,
it's entirely acceptable to SKIP this optional fusion and land only the
mandatory streaming-hash fix above. State explicitly in your report
whether you attempted the fusion and why you did or didn't land it.

## Tests (TDD)

- **Hash equality, streamed vs. whole-file, on a LARGE generated file**: a
  few MiB (e.g. 5-10 MiB) of generated content (deterministic, not random —
  reproducible test), computing the expected SHA-256 via the TEST's own
  independent call to `sha2::Sha256` (or the existing one-shot `sha256()`
  helper) over the whole buffer, then asserting the STREAMED
  `collect_manifest_entries`/`verify_manifest` computation (via a real
  `backup()`/`verify_manifest()` call over a temp dir containing this
  file) produces the IDENTICAL hex digest and byte count. This is the
  core regression proof.
- **Existing backup/restore/manifest suites** (`crates/shamir-server/src/tests/backup_tests.rs`,
  `crates/shamir-server/tests/backup_restore_e2e.rs`) must ALL stay green,
  UNCHANGED — this is a hash-computation-mechanism refactor, not a
  behavior change; every existing hash/checksum assertion must still pass
  bit-for-bit.
- If you land the optional copy+hash fusion, add a test proving `backup()`
  still produces IDENTICAL output (same files, same manifest) to what the
  unfused two-pass version would have produced — reuse/adapt an existing
  `backup_copies_all_files`-style test rather than inventing a parallel one.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Primary code area: `shamir-server`
(`backup.rs`, its `Cargo.toml` for the new `sha2` dependency, its tests).
Do NOT touch `restore.rs`'s CR-B7 reordering logic (ticket invalidation
before swap) — this task only changes HOW bytes are hashed/read, not
anything about restore's step ordering or manifest-hardening validation
logic (format_version/path-traversal/duplicate checks all stay exactly as
CR-B7 left them).
