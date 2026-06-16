בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# W2a+W2b — index-subsystem → RecordRef (план, byte-identical)

Из дизайн-пасса @aoh. КРИТИЧНО: индекс-ключи персистятся → byte-identity обязательна.

## Byte-identity КРУКС (W2a hash/unique)
Легаси-ключ = `FxHash(<InnerValue as Hash>::hash(leaf))` с ведущим
`std::mem::discriminant(Value<InternerKey>)` (value.rs:458). `ScalarRef` —
ДРУГОЙ enum (6 вариантов vs 10) → его discriminant иной → хеш разойдётся.
**РЕШЕНИЕ (Option A1, proof-by-construction):** в точке хеширования
МАТЕРИАЛИЗОВАТЬ leaf в InnerValue (`rec.materialize_at(path)`) и хешировать ЕГО
через НЕИЗМЕНЁННЫЙ `IndexRecordKey::with_values<T:Hash>(&[&InnerValue])`. Так байты
идентичны по построению. НИКОГДА не хешировать ScalarRef напрямую.
**materialize_at, НЕ scalar_at** для hash/unique — иначе Dec/Big/контейнер-поля
дают None → выпадают из индекса (тихая порча). materialize_at сохраняет любой leaf.

## W2a sorted (чистый случай)
sorted-ключ = `sort_codec::encode_*` (primitive-driven, НЕ InnerValue-структура).
`scalar_at(path)` отдаёт ровно {Null,Bool,Int,F64,Str,Bin}, None для Dec/Big/контейнер
— ТОЧНО совпадает со старыми match-арм. → `scalar_at` + sort_codec = byte-identical.
`build_covering_projection` → materialize_at (owned leaf, msgpack та же).

## W2b dyn IndexBackend → &(dyn RecordRef + Sync)
RecordRef object-safe. Параметры trait → `&(dyn RecordRef + Sync + '_)` (param-site,
не расширять trait-bound). Per-backend:
- Functional — БЕЗ изменений (уже `IndexExpr::eval(&impl RecordRef)`).
- FTS — `extract_text` → `str_at(path)` (идентично).
- Vector — `extract_vec` → `any_seq_elem(path, |sr| push f64/int)`; флаг: any_seq_elem
  принимает List И Set (вектора всегда List → безвредно).
- registry/apply_index_ops — без изменений (потребляют output-ops, не запись).

## Порядок (direct rewrite, старое УДАЛИТЬ; 3 коммита)
1. **W2a-sorted** (низкий риск): sorted_index_manager на scalar_at/materialize_at;
   планеры RecordRef-generic; удалить resolve_path_ref. Гейт: index sorted + covering.
2. **W2a-hash/unique** (крукс): index_keys.rs → `extract_index_leaves`(materialize_at)
   + `build_index_key`(with_values неизм.); удалить 5 старых fn; index_manager(_unique)
   RecordRef-generic; swap engine call-sites (doctor/crud/tx_ops). **Обязательный
   byte-identity gate-тест** (ключи old==new байт-в-байт + одинаковый BTreeSet<RecordId>
   по записи/типу вкл. Dec/Big/Map/List/composite). Гейт: index + @oracle + @engine
   index + crash_recovery. ТОЧКА НЕВОЗВРАТА (удаление старого API).
3. **W2b dyn-flip**: backend.rs params → &(dyn RecordRef+Sync); FTS/vector тела;
   swap call-sites (coercion). Гейт: index fts/vector/functional + index2_migration +
   index2_persistence + crash_recovery.
lookup_by_index / check_unique / read-path call-sites — БЕЗ изменений (литералы
&[InnerValue], не чтение записи).

## ЛАТЕНТНЫЙ блокер для W2c/W2d (флаг)
`RecordView::materialize_at` декодит Dec/Big как `Str` (лензa не различает на проводе).
В W2a/W2b запись ещё дерево → расхождения нет. Но когда лензa станет источником записи
(W2d), Dec/Big-ключевой индекс захеширует Str → разойдётся с историей. Gate-тест на
`tree.materialize_at(dec) == lens.materialize_at(dec)`; если падает — Dec/Big-индексы =
известное ограничение W2d, задокументировать.
