בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Лог автономных решений (Wave 2 + остаток цели)

Решения, принятые самостоятельно «в сторону красоты и совершенства» (по просьбе
пользователя), для финального отчёта. Принцип: чисто, полно, byte-identical,
без fallback, старые типы удаляем.

## Уже принято (до этого лога)
- **Gate-политика:** исполнитель (crush/агент) делает УЗКИЙ self-check (`-p`
  тронутых крейтов + точечные тесты), оркестратор — ОДИН авторитетный
  `clippy --workspace` + широкий тест-скоуп + diff-ревью + коммит. Убирает
  двойной прогон тестов; clippy-дубль и так cache-hit.
- **Параллельные crush в одно дерево — НЕ делать** (изменения смешиваются, общий
  Cargo.lock; изолированный коммит грязный). По одной crush в дерево.
- **Запуск crush — только `run_in_background`, без `&`/`sleep` в той же команде**
  (короткий баш убивает crush-ребёнка на выходе).
- **W1 dev-dep:** оставил `rust_decimal`+`num-bigint` test-only в shamir-engine
  (для Dec/Big в identity-тесте; те же версии, lockfile уже содержит) — flagged.
- **Stage 5 scope:** minimal (клиентский кеш, ZERO server change); id-ключевой
  провод + lazy-delta → отложено в #50 (выигрыш только байты провода, серверный
  интернинг уже дёшев — measured).
- **Wave 2 декомпозиция:** NO-GO на наивный cutover → по решению пользователя
  «довести до конца» → структурировал в эпик #45 (W2a IndexRecordKey → W2b
  IndexBackend → W2c StagedRow → W2d cutover), W2d ждёт a/b/c.
- **Частичный Stage 5 после kill:** добил (не выбросил) — компилился, структура
  была; crush-finish починил THasher/модуль/линты.

## (дописывается по ходу)

## Wave 2 — реализация (эпик завершён, коммиты)
- **W2a (byte-identity крукс):** легаси/unique индекс-ключи = `FxHash(<InnerValue as Hash>)`
  с ведущим `mem::discriminant(Value)`. Решение Option A1: `materialize_at`→leaf→
  НЕИЗМЕНЁННЫЙ `with_values`, НИКОГДА не хешировать `ScalarRef` (другой discriminant).
  `materialize_at` (не `scalar_at`) — иначе Dec/Big/контейнер выпадают из индекса.
  sorted-индекс — `scalar_at`+sort_codec (primitive-driven, арм-match). Коммиты
  e2abab5 (sorted), 7e60866 (hash/unique). byte-identity тесты обязательны.
- **W2b:** IndexBackend → `&(dyn RecordRef + Sync + '_)` (param-site, не trait-wide).
  FTS→str_at, vector→any_seq_elem, functional без изменений. Коммит ba53050.
- **W2c+W2d (точка невозврата):** StagedRow::Live/set_many_live/rewrite_set_inner
  УДАЛЕНЫ; staging хранит Bytes; remap→rewrite_set_bytes+remap_inner_value_bytes
  (холодный interactive-tx путь, implicit пропускает). execute_insert_tx → прямой
  энкодер query_value_to_storage_bytes (byte-identity 12/12, ccb8ac4) + insert_tx_many_bytes
  (RecordView для index/unique/vector/sorted). validators=run_validators_qv (W1).
  Dec/Big-инвариант: insert-QueryValue никогда не даёт Dec/Big/Set → линза==дерево для
  ключей. non-tx/update/delete — вне scope (W3 follow-up). Коммит 3f2f40a.
- **Решение combine vs split:** W2a sorted/hash раздельно (риск), W2c+W2d вместе (один
  cutover, как Stage 4). encoder отдельно (additive foundation).
- **Стиль гейта на cutover:** жёсткий — crash-seam (implicit+interactive) + @oracle +
  index byte-identity + @e2e; перепроверял сам (не доверял агенту на точке невозврата).
- **Гигиена:** убрал мусорные *.log, что crush оставил в корне.

## #50 Stage 5-wire — split-решение
- **ambient epoch-delta — GO, реализую.** Элегантный механизм пользователя (клиент шлёт
  epoch, сервер дослыает дельту); backward-compat, переиспользует entries_after. Убирает
  отдельные dump-round-trip'ы.
- **id-ключевой insert — ОТЛОЖЕН (honest NO-GO без бенча).** msgpack `QueryValue` в
  BatchOp::deserialize уничтожает бинарные ключи → нужен отдельный opaque-bytes op; и
  encode-skip НЕ материален (§9.4-валидация съедает; query_value_to_storage_bytes уже
  single-pass; Wave 2 уже убрал дерево). Реальный выигрыш — лишь обход double-
  materialization, спекулятивно → bench-first отдельной задачей, не строить сложность
  ради недоказанного. Красота = не плодить параллельный wire-формат без доказанного win.

## #43 clippy — split-решение (measure-first)
Аудит: Vec::new 555(+136 тестов), String::new 131 (капасити); HashMap<_,_,THasher> 55,
сырой HashMap::new/default 5 (хешер). Всего HashMap/HashSet-как-тип 140 в 11 крейтах.
clippy --workspace УЖЕ зелёный (named-ctor баны держат SipHash).
- **Хешер-airtight — ДЕЛАЮ** (явный ask пользователя «полностью забанить + обходные»):
  type-ban std HashMap/HashSet/RandomState; мигрировать ~140 на shamir_collections
  TFxMap/TFxSet (unordered) / TMap/TSet (ordered); #[allow] на alias-сайте; deny.
  Behavior-preserving (чистый alias-swap). Bounded, механический.
- **Капасити — ADVISORY (не deny)**: 555 Vec::new + 131 String::new = 686 сайтов,
  бо́льшая часть честно мелкие/неизвестного размера → deny = churn ради with_capacity(0).
  Красота = не плодить затычки. Документирую; dylint (UNSIZED_ALLOC) — airtight-опция
  на потом, по явному слову/бенчу. Named-ctor + advisory = 80/20.
