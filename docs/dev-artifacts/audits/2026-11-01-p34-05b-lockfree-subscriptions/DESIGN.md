# P34-05b subscription routing — implemented design

## Inputs and primitive limits

The binding source is `docs/dev-artifacts/prompts/remediation/p34/05b-client-lockfree-subscriptions.md` (owner decision: variant B). `Cargo.lock` resolves scc **3.8.4** (client manifest range `scc = "3.8"`). Local pinned source and README were inspected. scc's bucket-array stack and resizing are lock-free/non-blocking, but **each bucket has an internal read-write lock**. `entry_sync`, `remove_if_sync`, `retain_sync`, `iter_sync`, and `contains_sync` exist in 3.8.4. This design removes the crate's coarse subscription `StdMutex`; it does not claim formal end-to-end lock-freedom, and scc operations can wait on internal bucket synchronization.

`CLAUDE.md` §Code ideology prescribes `scc::HashMap<K,V,THasher>` for concurrent registries, an O(1) atomic mirror rather than `scc::HashMap::len()` (O(N)), and no hot-path std mutex. `client/subscription/` holds `handle.rs`, `registry.rs`, a `tests/` folder (manifest `mod.rs` plus `registry_tests.rs`) and a re-export-only `mod.rs`; `tests/client/` stays at seven direct entries. The old single file `client/subscription.rs` is replaced by this folder. Public API and constants remain unchanged.

## Representation and transitions

`SubscriptionRegistry` is an `Arc` held as `SubsMap`. It holds the same `Arc<ClosedState>` as `PendingRegistry`, a private `scc::HashMap<u64, RouteState, THasher>`, and `AtomicUsize early_total`. One enum value per ID excludes simultaneous Early and Live states:

- `Absent -> Early(VecDeque<PushEnvelope>)`: router has observed open, obtains the per-key vacant entry, reserves one global slot with bounded `fetch_update`, and inserts a one-element queue. Failed budget admission creates no key.
- `Early -> Early`: router owns the per-key occupied guard; if queue length is below 256 and CAS reservation succeeds, append one envelope. Otherwise drop newest and emit the existing debug diagnostic.
- `Absent/Early -> Live { sender, terminal }`: registration checks authoritative close, obtains the key entry, inserts/replaces Live, releases exactly the extracted Early queue length, and sends that FIFO queue while retaining the occupied entry guard. Thus later same-key Live sends cannot overtake flush. Replaced Live terminal senders are dropped, preserving current replacement behavior; an old handle's later Drop does not match the new channel.
- `Live -> Live route`: `try_send` occurs while holding the occupied per-key guard. Full uses the existing warning, Closed is ignored.
- `Live -> Absent`: handle Drop calls `remove_owned` (`remove_if_sync`) and compares sender `same_channel` within the atomic conditional operation; sender/terminal route is removed together only for that handle's channel.
- `Any -> Absent` at shutdown: first `ClosedState::publish` winner publishes cause then `retain_sync` drains routes, delivers cause to each terminal sender, and releases exact per-entry Early lengths. It does **not** store zero: delayed prechecked route operations can still reserve then self-clean after observing close. After all in-flight operations complete, the atomic total and sum of Early queue lengths are zero.
- Closed registration: no insertion; send the authoritative cause into the new handle's terminal channel and return an empty stream.

Constants remain `CLIENT_SUB_CHANNEL_CAP = 256`, `EARLY_BUFFER_CAP = 256`, `EARLY_BUFFER_TOTAL_CAP = 1024`.

## Budget and ordering invariants

All reservations are acquired under the per-key entry guard, only for Early append or a vacant Early insertion. A successful `fetch_update` predicate is exactly `count < EARLY_BUFFER_TOTAL_CAP`; checked bounded accounting therefore never exceeds 1024. Every successful reservation owns exactly one queue element. Registration and draining release the exact removed queue length; post-close route cleanup removes only Early and releases its queue length. `debug_early_total()` is test-only and sums queue lengths through `iter_sync`; no map `len()` is used. Aggregate admission failures leave no empty Early entry, avoiding unbounded key growth.

For one key, all appends, registration flush, and Live `try_send` are serialized by the scc occupied-entry guard, giving FIFO in the per-key serialization order. The real reader has one routing task, so that order is wire arrival order. Concurrent test routers have no externally predetermined order; tests assert no duplicate delivery and non-regressing sequence order only where the sequence/order is intentionally serialized.

No guard crosses `.await`: test hooks pause before/after synchronous registry operations, never while an scc guard is held. `SubscriptionHandle::Drop` remains synchronous. The existing `HookPoint` fixture's `StdMutex` remains `#[cfg(test)]` and extracts its receiver before awaiting.

## Close authority and ordering

There is one close state, not separate registry flags: `PendingRegistry.closed` and `SubscriptionRegistry.closed` point to the same `ClosedState`. `ClosedState::publish` sets the `OnceLock` cause first, then SeqCst-stores its single `AtomicBool`; readers use SeqCst loads. First cause wins. Subscription and request paths re-read this authority after their successful map transition and conditionally remove/clean their own state when closed.

The SeqCst publication store and operation loads/postchecks prevent the store-buffering case: if an operation's postcheck observes open, its preceding map transition is ordered before a later close publication; if publication wins, the postcheck observes closure and self-cleans. Because traversal is not a global snapshot, an entry inserted after the drainer has passed its key is handled by that operation's postcheck. This argument depends on scc's per-key synchronous operations and the shared close flag, not a whole-map snapshot. Early accounting is independent of close publication and uses AcqRel CAS RMWs and matching AcqRel decrements; no zero-store can race a delayed reservation.

## Race coverage and limitations

- Route vs register: vacant/occupied entry serialization picks whether the push joins Early or Live; registration flushes under key guard before later Live sends.
- Route vs route: same-key queue cap and append are serialized; distinct IDs contend only on exact aggregate CAS.
- Register vs shutdown: `ClosedState` cause precedes drain; successful registration rechecks after insert/replacement and removes its own Live route, sends its terminal cause, and returns an already-terminated handle if close won. Shutdown clears entries found by its traversal; no global zero-store races with delayed operations.
- Route vs shutdown: an operation begun before publication may complete inside its key operation; the drainer removes that state or its post-close cleanup removes Early and releases exact reservation. Routes starting after publication return without mutating.
- Drop vs re-register: `remove_if_sync` plus `same_channel` prevents a stale handle removing the replacement.
- shutdown vs shutdown: only the successful first publication drains; later shutdown calls preserve that cause.

The protocol relies on scc 3.8.4's synchronous entry/retain semantics and per-bucket synchronization, not an asserted global linearizable snapshot. It does not promise a fixed relative order for pushes issued concurrently on independent producer tasks.

## Tests and test seams

Added subscription behavior tests in `client/subscription/tests/registry_tests.rs`. The race seams are `route_test` (hooks after the closed pre-check and after the map transition) and `register_test` (hooks before and after the insert); both run the SAME private phase functions as production (`route_precheck/route_apply/route_cleanup`, `register_precheck/register_insert/register_postcheck`), so a hook pauses inside the real window, not around the whole call.

- `concurrent_routes_never_exceed_aggregate_cap_and_denied_keys_are_not_kept`: multi-thread routers check total ≤1024, exact debug sum, denied ID absent, and zero after shutdown. Counterfactual: an unconditional increment or retaining empty Early keys violates assertions.
- `per_key_cap_is_enforced_and_early_flush_precedes_live_route`: concurrent same-ID routes prove the 256 cap and no duplicate buffered envelopes; post-registration Live marker follows the drained queue. Counterfactual: absent per-key cap/flush ordering fails count/sequence checks.
- `stale_handle_drop_does_not_remove_re_registered_route`: route reaches replacement and its terminal sender receives shutdown cause. Counterfactual: unconditional stale-ID removal loses the marker/cause.
- `registration_after_shutdown_yields_terminated_handle_without_residue`: empty stream, exact cause, empty map and zero budget. Counterfactual: post-close insertion leaves residue or open handle.
- `route_shutdown_between_precheck_and_apply_leaves_no_residue`: the router passed the closed check, shutdown drains an empty map, then the router's transition creates an Early buffer; only `route_cleanup` can remove it and return its reservation. The test asserts the apply had not run while paused. Counterfactual: no post-close cleanup leaves a key and one reserved slot.
- `route_shutdown_between_apply_and_cleanup_leaves_no_residue`: the router's transition ran (key present, one reserved slot, asserted while paused), shutdown's drain removes it and releases the slot, then cleanup finds nothing. Counterfactual: a double release underflows the counter (debug assertion) or a missing release leaks the slot.
- `registration_shutdown_before_insert_self_removes_and_terminates`: shutdown lands after the registration's closed check but before its insert (the drain sees an empty map); the late insert must be removed by the registration's own postcheck, which also resolves its termination channel. Counterfactual: no postcheck leaves a live route in a closed client and an unresolved handle.
- `registration_shutdown_after_insert_terminates_exactly_once`: shutdown lands after the insert; the drain resolves the termination channel and the postcheck must not resolve it a second time (`try_recv` yields the cause once, then nothing).
- `route_registration_shutdown_stress_terminates_current_handles_once`: registrant, router, and shutdown tasks start together on a barrier. Every handle, whether registered before or after close, must terminate with first cause; delivered envelopes must be unique and per-ID non-regressing; after all tasks finish the map and early total are empty. This checks the concurrent lifecycle interleaving; it does not prove all scheduler interleavings.

Existing real-wire tests continue to verify FIFO flush, cap reuse, and shutdown. `demux_tests.rs` budget oracle now uses registry test accessors; live route setup uses `register`. `lifecycle_tests.rs` removes source-text mutex assertions and records why: those checked spelling, not atomicity. Existing behavior-preservation tests may also pass under the prior mutex implementation; no local tests were run and no red result is claimed. Behavioral tests for the races absent from the old implementation are counterfactual controls, but whether a particular old-code path fails is left to CI.

## Source integration

- `client/transport.rs`: existing cause-first `ClosedState` now uses SeqCst flag publication/check; `Lifecycle` remains `#[derive(Clone)]`; pending and subscription registries share its `Arc`. Shutdown invokes subscription drain before unchanged pending drain/watch/hint behavior. Reader push branch calls registry route.
- `client/api.rs`: `subscribe_push` delegates registration to the subscription registry, then constructs the unchanged public handle.
- `client/subscription/{mod.rs,registry.rs,handle.rs}`: private state enum/map, exact budget, conditional removal, public handle and preserved API documentation.
- `tests/client/{demux_tests.rs,lifecycle_tests.rs}`: converted old mutex access/oracles and replaced the source-text guard with a reason comment.

No new dependency. scc's internal bucket locks remain the documented formal lock-freedom limitation. Local gates for this stage are `cargo fmt -p shamir-client -- --check`, `cargo clippy -p shamir-client --all-targets -- -D warnings`, `cargo check -p shamir-client-node --all-targets` and the layout checker; the behavioural tests run only in CI.
