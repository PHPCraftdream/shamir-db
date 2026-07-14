בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# W2c+W2d — write-path tree elimination (план, GO)

Из дизайн-пасса @aoh. Storage-формат НЕ меняется (id-keyed msgpack). Recovery
декодит те же байты. Staged bytes = WAL body verbatim.

## Вердикт: GO, со split'ом
- **Implicit-tx (горячий путь): полностью БЕЗ дерева.** Энкодер → base-id байты;
  overlay пуст → remap пропускается (pre_commit.rs:167); индексы из RecordView.
- **Interactive-tx: без дерева на insert-пути;** холодный commit-remap переиспользует
  дерево внутри `remap_inner_value_bytes` (A.2a) — приемлемо (редко, новые имена полей
  в multi-statement tx). НЕ insert-way-station.

## Шаги (коммит между; byte-identical)
1. **W2d-encoder** (additive, reversible): `query_value_to_storage_bytes(qv, intern_fn)
   -> Bytes` в codecs/interned/messagepack.rs — streaming Serialize (QvInternedRef),
   зеркалит value.rs:61-98 для значений; КЛЮЧ карты = `InternerKey::serialize` (bin,
   minimal LE — из value.rs:91, НЕ сырой u64, НЕ де-интернинг как в messagepack.rs:401).
   **Обязательный byte-identity тест:** `query_value_to_storage_bytes(qv,f) ==
   query_value_to_inner_with(qv,f).to_bytes()` по батарее: Int width-boundaries
   (0/127/128/255/256/65535/65536/i64::MAX/негативы), F64(-0/NaN/inf), Str/Bin boundaries,
   nested Map/List/Set, **key-id width-boundaries (255→1b, 256→2b, 65536→4b, >2^32→8b)**,
   key-ORDER (несортированный → байты совпали). Dec/Big тоже (serialize_str).
2. **W2c** (reversible): remap call-sites (pre_commit.rs:178, tx_context.rs:630) →
   `rewrite_set_bytes(|b| remap_inner_value_bytes(b, remap))`. Live/set_many_live пока
   оставить. Поведение-сохраняющее.
3. **W2d-cutover** (ТОЧКА НЕВОЗВРАТА): `insert_tx_many_bytes(&[Bytes], tx)` (строит
   RecordView::new(&bytes) на строку, кормит уже-RecordRef index/unique/vector/sorted +
   set_many байты); execute_insert_tx строит Vec<Bytes> энкодером, зовёт его; validators
   = run_validators_qv(QueryValue, W1); result-echo = resolved_values. УДАЛИТЬ:
   StagedRow::Live, set_many_live, rewrite_set_inner, построение inner_values.
   Гейт: crash-seam (implicit + interactive-new-field) + @oracle --full + index byte-id.

## Dec/Big инвариант (FLAG, не блокер insert)
RecordView::materialize_at декодит Dec/Big как Str (на проводе str). НО insert-QueryValue
НИКОГДА не даёт Dec/Big/Set (QueryValue-источник + resolve_computed_record схлопывает Dec→Str
через inner_to_query_value). → дерево и линза видят Str, согласны. Тест-инвариант +
guard-коммент в insert_tx_many_bytes. ФОРВАРД-ХАЗАРД: если появится QueryValue-источник
с Dec/Big (msgpack-клиент), Dec/Big-ключевой индекс под линзой разойдётся — гейтить
debug-assert/typed-encoder тогда.

## Out of scope (W3 follow-up)
- non-tx execute_insert (тесты/прямой вызов; прод идёт через implicit-tx) — ещё дерево.
- update_tx/delete_tx (read-back старого дерева) — insert-only волна.

Файлы: messagepack.rs (+encoder), messagepack_tests.rs (+байт-id), staging_store.rs
(удалить Live/set_many_live/rewrite_set_inner), pre_commit.rs+tx_context.rs (remap→bytes),
table_manager_tx_ops.rs (+insert_tx_many_bytes), write_exec.rs (execute_insert_tx cutover).
