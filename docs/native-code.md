# Native code in the workspace — what C remains and why

Owner ruling: the workspace goes Rust-only where that comes easily; where no
real Rust alternative exists, the C/C++/assembly dependency stays and is
documented here. `scripts/ci/gates/native_code_check.py` (the `deny` CI job)
enforces this state: every crate that reaches a build tool (`cc`, `cmake`,
`bindgen`, `pkg-config`) must be in the gate's allowlist (table below), and the
deny-listed crates must never re-enter the built graph. An owner decision is
required to add a native dependency; the allowlist entry in the gate script
and an entry in this document must land in the same PR.

## KEEP-AS-IS — C that remains, one line per entry

| Crate | Why the C stays |
| --- | --- |
| `aws-lc-sys` (AWS-LC, C + assembly; prebuilt NASM objects on Windows x86_64) | rustls TLS 1.3 and certificate crypto. No audited production-ready pure-Rust provider exists (`rustls-rustcrypto` is alpha and marked not for production, `rustls-graviola` is new and unaudited, `ring` is C/asm too). Re-evaluate when graviola publishes an audit or reaches 1.0. |
| `wasmtime` `helpers.c` (Unix) | Part of wasmtime's runtime for native-code hosts; no Rust-only wasmtime build exists. WASM execution is a core product feature. |
| `wasmtime-internal-fiber` `windows.c` (Windows) | Stack switching behind wasmtime's async support, which our async host imports require. |
| `libproc` via `bindgen` (macOS build only) | Process metrics; binds to the OS libproc, no C is compiled; the only replacement would be hand-written unsafe FFI. |
| `generator` (loom feature only) | Assembly context switching for the loom model checker; built only in the CI loom job, never in a product build. |
| `napi-sys` (shamir-client-node only) | Node-API is a C interface by definition; nothing C is compiled; not part of the server binary. |
| Windows import libraries (`windows-targets`, `winapi-*-gnu`) | Prebuilt stubs for OS DLLs — link-time data, no compiled C. |

## Gate allowlist — direct dependents of each build tool

The gate runs `cargo tree -i <tool>` over the whole workspace (all
features, all targets, normal + build + dev edges) and compares the direct
dependents with this list in both directions: an unknown dependent and a
stale entry both fail.

| Build tool | Allowed direct dependents |
| --- | --- |
| `cc` | `aws-lc-sys`, `cmake`, `blake3`, `generator`, `iana-time-zone-haiku`, `wasmtime`, `wasmtime-internal-fiber` |
| `cmake` | `aws-lc-sys` |
| `pkg-config` | `aws-lc-sys` |
| `bindgen` | `libproc` |

The graph is an inventory over every feature and target, not a statement
of what one product build compiles. Entries that appear in it without
compiling C in a product build: `generator` (loom only),
`iana-time-zone-haiku` (Haiku targets only), `blake3` (its `cc`
build-dependency is inactive under the `pure` feature), `libproc` (macOS
only, no C is compiled), `cmake` (only a helper of `aws-lc-sys`).

Hard deny list — these must never be in the built graph, even though some
remain in `Cargo.lock` as unused entries: `ring`, `libmimalloc-sys`,
`zstd-sys`, `wasmtime-internal-jit-debug`, `openssl-sys`. Presence in the
`cargo tree` graph, not in the lockfile, is what fails.

## History

- #226 — blake3 switched to the pure-Rust `pure` feature (its `cc`
  build-dependency remains only as an unused lock entry).
- #227, #246–#250 — zstd removed; the cache rewrite replaced the `zstd-sys`
  based layer.
- #255 — the bench-only `mimalloc` option removed (`libmimalloc-sys` is gone).

## Rule for adding a native dependency

1. An owner decision is required (is there really no Rust alternative?).
2. The same PR adds the crate to the gate's `ALLOWLIST` in
   `scripts/ci/gates/native_code_check.py` and a row above.
3. If the crate is on the deny-list spirit (previously removed), it needs an
   explicit re-admission, not an allowlist entry.
