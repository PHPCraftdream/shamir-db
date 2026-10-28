# P1 stage H13b — p1-tx-overlay-names

## Goal

Card **RV-TYPES-97A84E4F** (`RecordRef::to_query_value` swallows de-intern errors into `QueryValue::Null`), split out of H13 (#361). Making record de-interning fail-closed exposes a pre-existing master bug: rows staged in an INTERACTIVE (non-implicit) transaction carry field ids interned into `tx.interner_overlay` (ids >= 2^48, `crates/shamir-tx/src/tx/support/layered_interner.rs`), written via `crates/shamir-engine/src/table/write/insert.rs`, and the read-your-own-writes path (`table/support/test_seams/tx_scan_overlay.rs` `overlay_rows_for_tx`) projects them with the plain base interner. Master hides this by returning Null rows (tests only counted rows).

Deliver BOTH together: fail-closed de-intern AND correct overlay name resolution on every interactive-tx read path.

## Starting material
- Commit `c9071712` (on the merged H13 history) is the revert of the fail-closed change — revert it back (`git show c9071712` for the exact hunks) to restore `Result`-returning `to_query_value` and `?` propagation.
- Commit `99b4c0d0` (branch `fix/p1-tx-overlay-names`) holds partial round-6 work: resolver-taking codec variants (`shamir-types/src/codecs/interned/codec.rs`), overlay snapshot in `query/filter/evaluation/runtime/core/eval_context.rs`, attach in `query/batch/runner/query_runner/runner.rs`, partial dispatch updates, regression `interactive_tx_read_resolves_overlay_interned_field_names` (interactive_tx_tests.rs), tightened interactive_deadline_tests, Windows access_meta fixture interner fix. Read it with `git show 99b4c0d0`.

## Remaining requirements (from the round-6 review)
1. Lower-level tx read APIs (`read_tx` / `read_for_tx` and any public table read that takes `Some(tx)`) must resolve overlay names without callers having to attach anything.
2. Projection / ORDER BY field paths must be resolved through the layered interner (`LayeredInterner::get_id`) on the tx path — today they are compiled against base before context binding.
3. Overlay name lookup must be O(1) (id → name map built once per read), not a linear scan.
4. Regression covers plain read AND `order_by_desc` on overlay-only fields, asserting row CONTENT; interactive_deadline_tests assert row content.
5. The four CI failures that made H13 fail must pass: shamir-server `interactive_tx_e2e::interactive_tx_happy_path_wire`, shamir-engine `interactive_deadline_tests` (both), shamir-db `access_meta_tests::resource_meta_group_fails_closed_on_storage_error` (Windows; fixture must share the repo interner).

shamir-engine now has `#![deny(clippy::unwrap_used, clippy::expect_used)]` (F14c): no new unwrap/expect in production code.
