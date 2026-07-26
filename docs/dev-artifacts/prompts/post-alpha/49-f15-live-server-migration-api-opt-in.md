# Brief for F-15 follow-up — live-server config opt-in for the experimental migration API

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context: a real gap surfaced by the TS e2e suite during the F-15 release gate

F-5 (#795, already merged) added `ShamirDb::enable_experimental_migration_api()`
(`crates/shamir-db/src/shamir_db/shamir_db/core.rs`) — `StartMigration` is now
rejected with `experimental_feature_disabled` unless this method has been
called on the `ShamirDb` instance first. This was the correct fix: the
migration feature has several known, documented gaps (no write interception,
non-durable coordinator state, count-only commit verification — see
`docs/guide-docs/KNOWN_LIMITATIONS.md` §2) that make it unsafe as an
always-on default.

However, `grep -rn "enable_experimental_migration_api" crates/shamir-server/`
returns **zero hits** — the live `shamir-server` binary (the only thing a
real client, including the TS SDK, ever talks to) has **no way at all** to
call this method. Running the TS e2e suite's full migration-lifecycle tests
(`crates/shamir-client-ts/src/__tests__/e2e-ddl.test.ts`, the two `it()`
blocks under "── 9. Migrations lifecycle ──", lines ~424-540) against a real
spawned `shamir-server` process now always fails with
`experimental_feature_disabled`, since there is structurally no way to opt
in. This task closes that gap: give the live server a config-file switch.

## Design (already decided — implement, don't re-derive)

**A single new config field, no CLI flag, no `serve()`/`ServerLauncher`
signature changes needed.**

1. **New field** in `ServerConfig`'s `SecurityConfig`
   (`crates/shamir-server/src/config.rs`, ~lines 219-254) — mirror the
   existing `ObservabilityConfig::allow_public_metrics` field's exact shape
   (~lines 157-182: `#[serde(default)]`, defaults `false`, explicit
   `Default` impl setting `false`):
   ```rust
   /// Opt into the experimental online storage-migration API
   /// (`StartMigration`/`CommitMigration`/`RollbackMigration`/
   /// `MigrationStatus`). DISABLED BY DEFAULT — this feature has several
   /// known, unresolved gaps documented in `KNOWN_LIMITATIONS.md` §2: no
   /// write interception (writes to the source table during an in-flight
   /// migration are lost from the destination), only the in-memory
   /// dst_engine is supported, and migration coordinator state is
   /// non-durable (a server restart mid-migration loses all state). Enable
   /// only for internal testing against a disposable data_dir — never in
   /// a production deployment.
   #[serde(default)]
   pub enable_experimental_migration_api: bool,
   ```
2. **Wire it at boot** in `crates/shamir-server/src/server/server_launcher.rs`'s
   `ServerLauncher::launch` — right after the existing construction block
   (~lines 366-372):
   ```rust
   let shamir = Arc::new(
       ShamirDb::init(SystemStoreConfig::Fjall(meta_path))
           .await
           .map_err(|e| BootError::ShamirDbInit(e.to_string()))?
           .with_user_admin_port(port)
           .with_principal_resolver(resolver),
   );
   ```
   Add, immediately after (before the existing `audit_store_b_vs_directory`
   call at ~line 378):
   ```rust
   if self.config.security.enable_experimental_migration_api {
       shamir.enable_experimental_migration_api();
       tracing::warn!(
           "experimental online storage-migration API is ENABLED — see \
            KNOWN_LIMITATIONS.md §2 for known gaps; do not use in production"
       );
   }
   ```
   (Adjust the exact `self.config...`/`launcher.config...` path to whatever
   this function's existing local binding for the config actually is — read
   the surrounding code first; do not guess the variable name.) Confirmed:
   this single `Arc<ShamirDb>` (via its internal `Arc<AtomicBool>` toggle,
   `core.rs` ~lines 132/557-570) is the SAME instance every wire-level
   `StartMigration` handler reads `experimental_migration_enabled()` from
   (`crates/shamir-db/src/shamir_db/execute/admin_migration.rs:47`'s
   `handle_start_migration`, via `ShamirAdminExecutor::shamir`) — no
   per-request/per-connection reconstruction exists, so this ONE boot-time
   toggle is sufficient for the entire process lifetime.
3. **No CLI flag.** There is no established CLI+config-with-precedence
   convention in this codebase to mirror (checked: RI-9's
   `--bootstrap-token-path` is CLI-only, no config counterpart) — adding a
   CLI flag here would require threading a new parameter through
   `main.rs`'s `Cli` struct, `runtime::serve`'s signature, and
   `ServerLauncher`, for no real benefit over a config field alone (a
   dangerous experimental flag belongs in a reviewed, versioned `.ktav`
   file, not an ad-hoc CLI override an operator might paste into a shell
   history). Do NOT add a CLI flag — config field only.
4. **Do NOT set this field in `deploy/server.example.ktav`,
   `server.small.example.ktav`, or `server.medium.example.ktav`** — it must
   stay `false` (the default, simply omitted) in every shipped example
   profile. Only the TS e2e test's OWN throwaway test-server config (see
   below) should set it `true`.

## Closing the TS e2e gap itself

Find how `crates/shamir-client-ts/src/__tests__/e2e-ddl.test.ts` (and its
shared harness, `e2e-harness.ts`) launches the real `shamir-server` process
for the test suite — it spawns the binary against some generated/fixture
`.ktav` config. Locate that config generation (likely in `e2e-harness.ts` or
a shared fixture file) and add
`security: { enable_experimental_migration_api: true }` (or wherever the
generated config's `security` block lives) to the config the test harness
generates, so the two migration-lifecycle `it()` blocks in `e2e-ddl.test.ts`
(lines ~424-540) can run against a server that has genuinely opted in —
their EXISTING bodies (full start→status→rollback and
start→commit→dst-readable→not_found flows) should need NO changes once the
server they're talking to has the flag enabled. Do not weaken or shrink
those two tests' assertions; the goal is restoring their ability to run
against a real server, not replacing them with a shallower check.

If the harness spawns ONE shared server process for the entire
`e2e-ddl.test.ts` file (check `describe.skipIf(!SERVER_AVAILABLE)(...)`'s
`beforeAll`/`afterAll` at the top of the file) rather than one per-test
server, confirm enabling this flag for that whole shared process is safe —
it should be, since every OTHER test in that file exercises unrelated DDL
ops (createDb/createTable/etc.) that don't touch migration at all, so
leaving the flag on for the whole file's shared server has no effect on
them.

## Tests

1. **Rust**: a new config test in `crates/shamir-server/src/tests/config_tests.rs`
   asserting the new field defaults to `false` when omitted from a parsed
   `.ktav`, and parses to `true` when explicitly set — matching this file's
   existing style for similar boolean-default assertions (e.g.
   `default_max_inflight_response_bytes_is_none`-style tests).
2. **Rust**: an integration-level test (check
   `crates/shamir-server/tests/` for an existing boot/config-driven
   behavior test to extend, or add a small new one) confirming a server
   booted with `security.enable_experimental_migration_api: true` in its
   config accepts a `StartMigration` call that would otherwise be rejected
   — a direct regression guard for the wiring in `server_launcher.rs`, not
   just the config parsing.
3. **TS**: re-run the FULL `crates/shamir-client-ts` test suite (`npx
   vitest run`, with `SHAMIR_SERVER_BIN` pointed at a freshly-built debug
   binary — see this repo's `CARGO_TARGET_DIR` env var, likely
   `D:\dev\rust\.cargo-target\debug\shamir-server.exe`) and confirm BOTH
   previously-failing `e2e-ddl.test.ts` migration tests now pass with their
   ORIGINAL assertions intact, and confirm no other test in the suite
   regressed (should stay at "53 files / 1005 tests" or grow by the new
   Rust-side tests' TS-visible effects, i.e. unchanged TS test count).

## Constraints

- Do NOT change `ShamirDb::enable_experimental_migration_api()`'s own
  signature or behavior (`shamir-db` crate) — this task only wires an
  EXISTING method into the live server's boot path.
- Do NOT add a CLI flag (see rationale above) — config field only.
- Do NOT set the new field to `true` in any shipped `deploy/*.ktav` example.
- Do NOT touch `admin_migration.rs`'s gate-check logic itself.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`) for Rust,
  never raw `cargo test`; `npx vitest run` (or whatever
  `crates/shamir-client-ts/package.json`'s `test` script invokes) for TS.
- `cargo fmt -p shamir-server` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean.
- Follow workspace conventions: `use` at file top, surgical diff, no
  incidental refactors of `server_launcher.rs`/`config.rs` beyond what this
  task needs.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- config
./scripts/test.sh -p shamir-server --full
# then, from crates/shamir-client-ts, with a freshly built debug binary:
SHAMIR_SERVER_BIN=<path-to-fresh-debug-shamir-server.exe> npx vitest run
```
