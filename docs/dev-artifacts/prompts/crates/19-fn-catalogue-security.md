# Brief: function catalogue security windows (SEC-FN-META)

## Why

A security review of the nested-call hardening (`crates/18-call-hardening.md`, landed) found four defects that predate it
and that nested calls now make easier to reach. All are in the function catalogue write paths and the root
invocation path of `shamir-db`:

* **F6 (high).** `create_function` with `replace` needs only `Function Write` on an existing function, keeps the existing
  resource metadata (owner, mode) and stores the NEW `opts.security`
  (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/function/management.rs`, the `opts.replace` branch around
  lines 256-300). Any actor with `Write` on a function (for example a legacy row without `mode`, which loads as 0o777) can
  replace its code and mark it `Definer`; it then runs as the owner or as `System`. The converse is the same hole:
  `Write` on an already `Definer` or setuid function lets a non-owner substitute code that runs with the owner's rights.
* **F4.** `invoke_function_root` (`db/function/invocation.rs`) treats a missing catalogue row as the open
  `ResourceMeta` (mode 0o777) and falls back to the in-memory `function_meta` secret/network grants (`core.rs`
  `build_invoke_ctx`). The rename (`management.rs`, between the authorization and the catalogue move) and drop
  windows produce exactly this state for a catalogue-managed function, so any caller can run it as itself with its
  grants. Registry-only functions also get the full database network allowlist.
* **F3.** The catalogue row is written first and the registry entry replaced afterwards (`management.rs` save, then
  `self.functions.replace`). The capture -> catalogue read -> recapture check of the root and nested paths does not
  detect this: the registry still holds the old artifact while the row already carries the new security, secret
  grants and network grants, so old code can run with new rights.
* **F5.** `FunctionRegistry::invoke` / `FnCtx::call_stored` without a resolver take the unchecked catalogue-free
  fallback. It is only for host-only tests, but the public `ShamirDb::functions()` makes it reachable from code that
  has a `ShamirDb`.

## What (decisions are made; do not re-open them)

1. **F6 rule.** A replace is allowed only if either (a) the actor is the function's owner (as `owner_field` of the
   existing record) or an administrator (`Actor::System`, `Actor::Admin`, or whoever the existing code treats as
   `Manage` on the root), or (b) neither the existing record nor the new options is privileged: the existing function
   is `Invoker` without the setuid bit AND the new `security` is `Invoker`. Everything else (non-owner replacing a
   `Definer`/setuid function, or any actor switching a function to `Definer`, or a row without a readable owner) is
   refused with an access error before anything is written. Reuse the existing owner/admin decision helpers; do not
   invent a parallel ACL. Replacement keeps retaining the existing resource metadata.
2. **F4 rule.** At the root, a missing catalogue row is allowed only for a function that has neither an in-memory
   `function_meta` entry nor secret/network grants, i.e. a built-in (`argon2id`-style). If `function_meta(name)` exists
   but the row is missing, refuse. A registry-only function never receives the database network allowlist; it gets the
   empty/deny gateway.
3. **F3 rule.** Tie the record to the artifact. Add a catalogue field (name it, e.g. `artifact_id`) holding a fresh
   unique value generated at every create/replace and the same value stored in the registry entry by `register` /
   `replace` (an extra, crate-visible parameter or a sibling API; keep the public `register`/`replace` signatures
   working for host-only callers, which get `None`). The root path and the nested resolver require
   `record.artifact_id == binding.artifact_id` after the capture/read/recapture steps; a mismatch is a fail-closed
   "function changed during resolution" error. Rows and registry entries without an id (legacy) compare equal as
   `None == None`; a row with an id and an entry without one (or the reverse) is a mismatch. Rename moves the
   entry and keeps its id; boot-time materialisation of persisted functions must restore the id from the row.
   Mind the order of the two writes: with this check, either order is safe; say which you keep.
4. **F5 rule.** The registry gets an immutable "catalogue-bound" flag set by `ShamirDb` when it builds its registry.
   `call_stored` / `FunctionRegistry::invoke` on a catalogue-bound registry without a resolver (or without an active
   captured root) fails closed. Host-only registries keep the existing behaviour. Do not make a new public setter
   callable after construction.

## Tests (they run in CI, not here)

Native-function tests in `crates/engine/shamir-db/tests/functions/` (look at `native_stored_call/` and the
lifecycle tests for helpers), each its own test: F6 (non-owner Write cannot switch to Definer; non-owner cannot replace
a Definer or setuid function; owner can; admin can; a plain Invoker -> Invoker replace by a Write holder still works;
legacy row without mode/owner by a non-admin is refused); F4 (missing row with function_meta present is refused at the
root, a built-in without grants still works, a registry-only function gets a deny gateway); F3 (replace between capture
and read — drive it through a test hook or by interleaving writes, whichever the existing registry tests use —
fails closed; a normal replace followed by a call runs the new artifact with the new metadata; rename keeps the id;
reboot restores the id); F5 (catalogue-bound registry refuses the unchecked path; host-only registry unchanged).
Existing tests: never delete or weaken; list every changed one with the reason.

## Observable acceptance

1. All of the above implemented and tested; expectations come from the rules above.
2. `cargo fmt -p <crate> -- --check`, `cargo check -p <crate> --all-targets` and
   `cargo clippy -p <crate> --all-targets -- -D warnings` clean for every touched crate (`shamir-wasm-host`,
   `shamir-db`, `shamir-engine` as touched).
3. `python -I scripts/ci/check_layout.py` passes (at most 7 entries per folder, 1000 lines per file; split rather than
   grow; `management.rs` is large: put new logic in a new file).
4. rust-guard rules: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no
   visibility widening beyond what is strictly needed (list every one).

## Non-goals

`call_checked`, signature metadata, the language, gateway-originated `Call`, fuel changes, version changes, any
change of the nested-call rules other than the four above.

## Constraints

Compilation, builds and tests run in CI, not here. The only allowed commands are
`cargo fmt -p <crate>`, `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings` and `python -I scripts/ci/check_layout.py`, plus read-only
git. Run cargo only under `wnice -p idle -c 50 -t 8 -m 50 --` and with `-j 2`. No `cargo build/test/run/bench/doc`,
no nextest, no npm. Run `cargo fmt -p <crate>` for every touched crate before the final report.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

Your last message must be a final report (do not end on a tool call): what is implemented, the field and id scheme,
write order kept, every widened item, every existing test changed and why, anything unverified.
