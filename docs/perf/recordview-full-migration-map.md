בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# RecordView FULL migration map (read path, NO InnerValue fallback)

Design from a read-only survey (`@aoh`). Decision: full migration — the hot
read path stops building the `InnerValue` tree; consumers read via `RecordRef`;
where an owned value is unavoidable it is materialised **for one value only**
(`materialize_at`), never the whole record. Companion to `record-view-migration.md`.

## Orientation facts
- Storage is already id-keyed msgpack (`InnerValue::to_bytes`, keys = `InternerKey` as `bin`). Job = stop building the tree, not re-key.
- The tree is produced in ONE place on the read path: `Table::decode_raw_batch` / `decode_raw_batch_filtered` → `InnerValue::from_bytes(value_bytes)` (`crates/shamir-engine/src/table/table.rs:266`, `:326`). **This is the Stage-4 point of no return.**
- `eval_bytes.rs` = existing tri-state byte pre-filter (reject-fast); orthogonal, folds in post-cutover.
- Index-extract + validators are **WRITE-path** → separate later **Wave 2**, do NOT gate the read cutover.
- Aggregates/GROUP BY legitimately keep an owned tree (`raw_acc` reused for sort/group) — do NOT force onto the lens.

## B. Full `RecordRef` trait (chosen: typed accessors + seq visitor + single-value materialise)
Required primitives: `scalar_at`, `present_kind_at`, `any_seq_elem`, `materialize_at`, `for_each_field`. Defaulted: `scalar`, `str_at`, `exists_at`, `is_null_at`.
```rust
pub trait RecordRef {
    fn scalar_at(&self, path: &[InternerKey]) -> Option<ScalarRef<'_>>;            // (exists)
    fn scalar(&self, id: InternerKey) -> Option<ScalarRef<'_>> { self.scalar_at(&[id]) }
    fn str_at(&self, path: &[InternerKey]) -> Option<&str>;                         // Like/Regex/Fts (default off scalar_at)
    fn present_kind_at(&self, path: &[InternerKey]) -> Option<Kind>;                // REQUIRED low-level: Null|Scalar|Container|NonComparable
    fn exists_at(&self, path: &[InternerKey]) -> bool;                             // default off present_kind_at
    fn is_null_at(&self, path: &[InternerKey]) -> bool;                            // default off present_kind_at
    fn any_seq_elem(&self, path: &[InternerKey], f: &mut dyn FnMut(ScalarRef<'_>) -> bool) -> Option<bool>; // Contains*
    fn materialize_at(&self, path: &[InternerKey]) -> Option<InnerValue>;          // single value escape hatch (registry/$fn/sub-object projection)
    fn for_each_field(&self, f: &mut dyn FnMut(InternerKey, InnerValue));          // SELECT * / projection
}
```
- All `&self` → static `&impl RecordRef`, zero vtable on the hot quartet (`scalar_at`/`str_at`/`exists_at`/`is_null_at`). `&mut dyn FnMut` closures = one indirect call **per element**, paid only by Contains/projection (cold relative to Compare).
- `materialize_at` is the ONLY allocating method; confined to registry/computed/sub-object/InSet-LHS.
- Lens supplies every building block: `get_path` (scalar/str/kind), `RawSeq::iter` (any_seq_elem), `fields` (for_each_field). `materialize_at` for the lens = decode ONE subtree (new lens→tree bridge, reuse `read_value`/`msgpack_to_inner` per subtree). `InnerValue` impl = trivial tree descent / one-subtree clone.

## C. Migration order (each = its own commit; tree still flows C0-C8 → behaviour identical)
- **C0** — extend trait + both impls + parity unit tests. shamir-types only. Gate `@types` (./scripts/test.sh -p shamir-types).
- **C-sig** — flip `FilterNode::matches(&self, record: &impl RecordRef, ctx)` (was `&InnerValue`). NO-OP (InnerValue: RecordRef → all call sites monomorphise unchanged). Keep `FilterCallback::matches(&InnerValue,...)` as a concrete shim. Gate `@oracle`.
- **C1** — `Exists/NotExists/IsNull/IsNotNull` → `exists_at`/`is_null_at`. Gate `@oracle`.
- **C2** — `Compare`/`Between`/`In`(list) → `scalar_at` + `scalar_ref_cmp(a:ScalarRef,b:&InnerValue)` (already in scalar_ref.rs:78, mirrors compare_values incl. Int/F64 cross). `InSet` → `materialize_at` LHS to keep O(1) `TSet::contains` (§E.1). RHS stays owned. Gate `@oracle`. (split Compare/Between vs In/InSet into 2 commits.)
- **C3** — `Like`/`Regex`/`FtsMatch` → `str_at`. Gate `@oracle`.
- **C4** — `Contains`/`ContainsAny`/`ContainsAll`(+Set) → `str_at` (str case) / `any_seq_elem` + `scalar_ref_cmp` (List/Set). Gate `@oracle` + engine `--full` (collection_tests).
- **C5** — projection `SelectProjection::project_value`/`project` → per-field `materialize_at` then existing converters; `is_all` → `for_each_field`. Gate engine `--full` (select_projection_tests).
- **C6** — `ComputedCompare`/`$fn`: feed registry/`IndexExpr::eval` from `materialize_at` per arg/leaf (Route 1, §D). Registry signature UNTOUCHED. Gate `@oracle` + engine `--full`.
- **C7** — MIN/MAX shortcut (read_exec.rs:130/:337) → `scalar_at`/`materialize_at`. Fold into C5 or separate.
- **C8** — aggregates: LEAVE `raw_acc: Vec<(RecordId, InnerValue)>` owned (correct; reused for sort/group). HAVING `matches` already migrated. Optional.

Hot-path risk: `Compare` (C2) is THE hot loop. While tree flows, cost is identical (InnerValue map lookup O(1)). The per-atom-rescan question only bites after cutover → answered by building `RecordView::index()` once per record at Stage 4 and passing a FieldIndex-backed view. `scalar_ref_cmp` MUST stay arm-for-arm == `compare_values` (parity tests).

## D. Scalar-registry seam ($fn / ComputedCompare)
Registry `ScalarFn = Arc<dyn Fn(&[InnerValue]) -> ScalarResult>` (funclib/registry.rs:44,:130) — **do NOT change signature**. Route 1 (chosen): in `resolve_filter_value` FnCall/FieldRef arms (resolve.rs:114,:123) swap `resolve_field` → `materialize_at(path)` — one owned value per field-arg, never the tree. `IndexExpr::eval(&InnerValue)` (shamir-index/expr.rs:54) gets an `eval_ref(&impl RecordRef)` using `materialize_at` for its `Field`/`JsonPath` leaves; keep `eval(&InnerValue)` as a shim. Route 2 (borrow-ify registry to `&[ScalarRef]`) REJECTED: breaks all of funclib, saves nothing (containers/nested-$fn still need owned).

## E. Hard cases (honest)
1. `InSet`/`In` vs `TSet<InnerValue>`: borrow can't probe owned-keyed set → materialise the ONE LHS field scalar, keep O(1) `set.contains(&owned)`. (`In`-literal-list uses `scalar_at`+`scalar_ref_cmp`, no materialise.)
2. Projection of nested object/array field → `materialize_at` one subtree (sanctioned). `SELECT *` (`is_all`) → `for_each_field` materialises each top value = effectively whole record, but only for `SELECT *` (which returns everything anyway; lens win is zero there by nature — honest).
3. `Like/Regex/Fts` → `str_at` zero-copy buffer borrow. Clean.
4. `Contains` List/Set elements that are containers/Dec/Big → surface as non-matching (matches current None-compare). collection_tests cover.
5. Deep `ComputedCompare` (Concat/Coalesce multi-field) → one `materialize_at` per leaf, bounded by expr size.
6. Aggregates → owned tree by design (collected, reused). Stage-4 `needs_raw` arm still produces InnerValue.
7. Index-extract / validators = WRITE path → Wave 2, separate, does not gate read cutover.

## F. Cutover (Stage 4) shape
- Flip `decode_raw_batch_filtered`/`decode_raw_batch` (table.rs:247-289,:301-337): keep `value_bytes` (refcounted `Bytes`), yield a `RecordView`-bearing item for streaming/counting/non-raw-collecting paths → NO tree built. `needs_raw` (group-by/agg) arm still `from_bytes` → tree (§E.6).
- Batch item `Vec<(RecordId, InnerValue)>` (read_exec.rs:25-31 DynBatchStream) → carries `Borrowed(Bytes)` | `Owned(InnerValue)`. Consumers already generic (`&impl RecordRef`) → filtered/narrow-projection build `RecordView::new(&bytes)` per row.
- Lifetime: `RecordView<'a>` borrows the batch `Bytes`; per-row filter+project completes within the batch loop (synchronous). No view escapes; `materialize_at` bridges anything outliving the row. `stream!` macro must own `Bytes` while view borrows it within one row iteration — care at impl time.
- POINT OF NO RETURN: the commit where the filtered scan yields borrowed `Bytes` instead of `InnerValue`. Isolate to one commit (decode_raw_batch* + DynBatchStream item + the 3 read_* loops read_exec.rs:520-538/:630-663/:723-757).
- Gates: `@oracle` + engine `--full` (projection/aggregate/computed/collection) + a temporary tree-vs-lens parity test asserting byte-identical `QueryResult` over a query battery. Durability untouched (storage bytes = what the lens reads).

## Key file:line index
- Trait/impls: record_view/record_ref.rs:24 (trait), :69/:92 (impls); scalar_ref.rs:78 (`scalar_ref_cmp`); lens.rs get_path/fields; record_value.rs:83 (`RawSeq::iter`).
- Filter atoms: query/filter/filter_node.rs:158 (`matches` sig), per-atom lines in design A1 (Compare:174, IsNull:205, InSet:214, In:230, Like:283, Contains:290, ContainsAny:327, ContainsAll:361, Between:403, Exists:443, Fts:446, ComputedCompare:504).
- RHS/registry: query/filter/resolve.rs:102/:114/:123; funclib/registry.rs:44/:130 (no change).
- Projection: query/read/select_projection.rs:75/:84/:91/:109/:117/:127.
- Computed: shamir-index/expr.rs:54/:132.
- Aggregates (leave on tree): query/read/aggregate.rs:131/:505.
- MIN/MAX: table/read_exec.rs:130/:337.
- CUTOVER: table/table.rs:266/:326; read_exec.rs:25-31 (batch alias), :520-538/:630-663/:723-757 (loops).
- Post-cutover fold-in: query/filter/eval_bytes.rs:626.
