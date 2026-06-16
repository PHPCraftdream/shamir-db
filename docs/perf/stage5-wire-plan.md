בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# #50 Stage 5-wire — план (split: ambient-delta GO, id-keyed bench-gated)

Из дизайн-пасса @aoh.

## Решающий факт
Сервер re-парсит insert через serde_json::Value (batch_op.rs:215), который
УНИЧТОЖАЕТ бинарные map-ключи. → id-ключевой insert НЕ может ехать в InsertOp.values;
нужен отдельный opaque-bytes op. И encode-skip НЕ материален: query_value_to_storage_bytes
уже single-pass, §9.4 id-валидация (get_str на ключ) съедает экономию. Реальный выигрыш —
только обход serde_json-double-materialization (спекулятивно). Wave 2 УЖЕ убрал дерево.

## ЧАСТЬ A — ambient epoch-delta (GO, делаем)
«Клиент шлёт максимум — сервер дослыает». Backward-compat, переиспользует entries_after.
- BatchRequest: `interner_epochs: TMap<String,u64>` (repo→epoch), #[serde(default, skip_if empty)].
- BatchResponse: `interner_delta: TMap<String,InternerDelta>`, InternerDelta{epoch:u64,
  entries:Vec<(u64,String)>}, #[serde(default, skip_if empty)].
- Сервер (BatchResponse assembly в query_runner/ShamirDb::execute): для каждого repo в
  interner_epochs → repo_interner().entries_after(epoch) → (entries, new_high) → attach.
  Машинерия как в admin_interner.rs:76-83.
- Клиент (Client::execute): заполнить req.interner_epochs из кеша для repos батча
  (collect_field_names repo-walk); после ответа — merge interner_delta (insert_entry +
  set_epoch, §9.4-safe). Epoch-гонка benign (set_epoch CAS-max, merge идемпотентен).
- Multi-repo батч: keyed by repo обязательно (не скаляр).
- serde_bytes для record-байт (bin, не array-of-int).
Гейт: query-types serde + engine/db + client + @oracle + e2e ambient (один клиент touch'ит,
другой на след. запросе получает дельту в ответе без явного dump).

## ЧАСТЬ B — id-keyed insert (ОТЛОЖЕНО, bench-first)
Если делать: новый InsertIdKeyedOp{insert_into_id: TableRef, records: Vec<serde_bytes>},
дискриминатор has("insert_into_id"); сервер validate_id_keyed_keys (walk map-ключей,
get_str на каждый id, reject client-invented §9.4) → insert_tx_many_bytes напрямую; клиент
прозрачно: touch unknowns → query_value_to_storage_bytes(cache) → InsertIdKeyedOp; ответ —
insert echo уже name-keyed (resolved_values), decode не нужен. ГЕЙТ НА БЕНЧ: wide-record/
large-batch implicit-tx insert before/after; если плоско — НЕ мёрджить (encode-skip съеден
валидацией, Wave 2 уже выиграл). Backward-compat: старый сервер → "Unknown operation type"
→ клиент fallback на name-keyed.
Deferred далее: TS client, upsert/update id-keyed, read-path id-filters, interactive-tx.
