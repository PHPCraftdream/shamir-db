בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Перф-ревью «радикальные ускорения» — O-нотация, скрытые сканы, аллокации (storage / wal / index-legacy / types / server / funclib)

_Агент: @fm, 2026-07-06. Дополнение к `2026-07-06-perf-hot-paths.md` (таски #441/#445) — покрывает крейты и подсистемы, которые тот аудит НЕ трогал: внутренности `shamir-storage` (fjall/cached), `shamir-wal` (segment set), legacy-индексы `shamir-index`, интернер `shamir-types`, сетевой слой, `shamir-funclib`. Ничего из находок 1.1–4.5 того аудита здесь не повторяется._

Все ссылки — по рабочему дереву на дату аудита.

---

## 1. ОБЯЗАНЫ УЛУЧШИТЬ (per-op перф-баги)

**1.1 [HIGH] FjallStore: memcpy всего значения на каждое чтение — при том что fjall отдаёт Arc-backed slice**
`D:\dev\rust\shamir-db\crates\shamir-storage\src\storage_fjall.rs:163` (`get`), `:293` (`get_many`), `:353` (`iter_stream` — ключ И значение), `:212-215` и `:428-431` (range/prefix-сканы): каждое чтение делает `Bytes::copy_from_slice(&slice)`. fjall 3.x `Slice` — это refcounted-байты; крейт имеет фичу `bytes`, при которой `Slice` == `bytes::Bytes` и конверсия **zero-copy** (`Bytes::from(slice)` без memcpy). Мы фичу не включаем (`crates/shamir-storage/Cargo.toml:32` — `fjall = { version = "3.0.1", optional = true }` без features).
Платим: +1 alloc + memcpy(размер записи) на каждый point-read; на сканах — ×2 (ключ+значение) на каждую строку. Для full-scan таблицы в 1 GB это гигабайт лишнего memcpy.
Как ускорить: включить `features = ["bytes"]` у fjall и заменить `copy_from_slice` на zero-copy конверсию.
Выигрыш: **×1.2–2 на disk-read-heavy пути**, минус alloc/op. Сложность: низкая (фича + точечные замены).

**1.2 [HIGH] FjallStore: `contains_key` + `insert` = двойной LSM point-lookup на каждую запись**
`storage_fjall.rs:114-123` (`insert`), `:142-148` (`set`), `:307-313` (`remove`): перед каждой мутацией — отдельный `contains_key`, т.е. полный LSM-лукап (memtable → bloom → уровни) поверх самой мутации. Комментарий §B13 обсуждает только TOCTOU-корректность, но не стоимость: **point-write платит ×2**. Для `insert` проверка вообще бессмысленна (RecordId — свежий 128-битный random, коллизия ~2⁻¹²⁸ — сам комментарий это признаёт). Для `set`/`remove` флаг `existed` нужен редким вызывающим — а платят все.
Как ускорить: (а) в `insert` убрать проверку целиком; (б) для `set`/`remove` — вариант API без `existed` (движок поверх MVCC и так знает существование), либо метаданные из fjall (если insert возвращает prior).
Выигрыш: **до ×2 на point-write** в fjall-бэкенде. Сложность: низкая.

**1.3 [HIGH] CachedStore: «стримы» жадно материализуют весь кэш/префикс до первого элемента**
`D:\dev\rust\shamir-db\crates\shamir-storage\src\storage_cached.rs:279-295` (`iter_stream`) и `:304-311` (`scan_prefix_stream`): ПЕРЕД созданием стрима собирается `Vec<(RecordKey, Bytes)>` со **всеми** записями (clone ключа + Bytes на каждую), и только потом нарезается на батчи. Потребитель с `LIMIT 10` всё равно оплачивает O(N) клонов + O(N) alloc. `TreeIndex::range` умеет резюмироваться по последнему ключу — тот же курсорный приём, что `storage_fjall.rs::iter_stream` уже делает.
Платим: O(N) alloc+clone на каждый скан, независимо от того, сколько реально прочитали; пиковая память = вся таблица.
Как ускорить: батч под epoch-guard `range(last_key..)` → yield → следующий батч (снапшотная согласованность TreeIndex это позволяет).
Выигрыш: скан с ранним выходом **O(N) → O(прочитанного)**; память O(batch). Сложность: низкая-средняя.

**1.4 [MED-HIGH] CachedStore.transact: инвалидация вместо апдейта — каждый закоммиченный ключ читается с диска заново**
`storage_cached.rs:326-345`: после `inner.transact(ops)` все затронутые ключи **удаляются** из кэша. Но коммит-путь движка пишет именно батчами `KvOp::Set` — т.е. первый read-after-write каждой свежезаписанной записи гарантированно промахивается в кэш и идёт в бэкенд (диск). Для read-your-writes-нагрузки кэш систематически холодный ровно на самых горячих (только что изменённых) ключах.
Как ускорить: применять `KvOp::Set(k, v)` к кэшу (значение уже в руках!), `Remove` — удалять. Значения — те же Bytes, лишней памяти нет.
Выигрыш: read-after-write из RAM вместо диска — **×10–100 на такое чтение**. Сложность: низкая.

**1.5 [MED] IndexManager: hit в posting-кэше глубоко клонирует весь BTreeSet на каждый lookup**
`D:\dev\rust\shamir-db\crates\shamir-index\src\legacy\index_manager.rs:634-636`: `return Ok((**cached).clone())` — полный клон `BTreeSet<RecordId>` (node-by-node alloc) на **каждый** equality-lookup; miss-путь дополнительно клонирует при заполнении кэша (`:669`). Для низкокардинального индекса (`status = 'active'`, 100k постингов) каждый запрос платит 100k аллокаций узлов дерева — кэш «ускоряет» скан диска, но сам стоит O(|postings|).
Как ускорить: сменить возвращаемый тип на `Arc<BTreeSet<RecordId>>` (вызывающие в `read_exec` только итерируют/пересекают), или на `Arc<[RecordId]>` (см. 3.2).
Выигрыш: lookup по горячему значению **O(|postings|) → O(1)**. Сложность: низкая-средняя (правка сигнатуры по цепочке).

**1.6 [MED] funclib: `distinct` — O(N²) с полным `PartialEq`-сравнением на элемент**
`D:\dev\rust\shamir-db\crates\shamir-funclib\src\arrays.rs:145-161`: `if !out.iter().any(|kept| kept == e)` внутри цикла — квадратичное сравнение `QueryValue` (для строк/вложенных структур каждое сравнение само O(len)). Массив в 10k элементов = 50M сравнений значений в одном вызове скалярной функции.
Как ускорить: hash-based dedup (ключ — канонический хэш `QueryValue`; для не-хэшируемых (F64) — фолбэк через сортировку/тотальный порядок).
Выигрыш: **O(N²) → O(N)**; на 10k элементов — порядка ×1000. Сложность: низкая.

---

## 2. ДЕГРАДАЦИИ ПОД НАГРУЗКОЙ / НА СТАРТЕ

**2.1 [MED-HIGH] SegmentSet::open: полный replay каждого sealed-сегмента ради одного числа**
`D:\dev\rust\shamir-db\crates\shamir-wal\src\segment_set.rs:131-141`: для каждого sealed-сегмента при открытии выполняется `seg.replay().await` (полное чтение + декод **всех** записей) только чтобы вычислить `max_version`. Startup-стоимость = O(суммарного объёма WAL на диске); при большом хвосте недотранкейченных сегментов (например, после долгого downtime) открытие БД читает и декодирует гигабайты, чтобы выбросить всё, кроме max.
Как ускорить: писать `max_version` в footer сегмента при seal (один 8-байтовый append перед fsync в `seal_and_rotate`, `:212`), либо sidecar-файл `NNNNNNNN.meta`; replay — только как фолбэк для сегментов без footer.
Выигрыш: open **O(WAL bytes) → O(#сегментов)**. Сложность: средняя (формат + backward-compat фолбэк).

**2.2 [MED] SegmentSet::replay / recovery собирает весь WAL в память одним Vec**
`segment_set.rs:235-252`: `out.extend(seg.replay())` по всем сегментам — recovery держит **все** записи WAL в RAM одновременно. Для WAL в несколько ГБ (макс. окно до truncation) это пиковая память O(WAL) и лишний полный проход.
Как ускорить: стриминговый replay (`impl Stream<Item = WalEntryV2>` посегментно) — потребитель (`repo_instance` recovery) применяет записи по мере чтения.
Выигрыш: recovery-память **O(WAL) → O(1 batch)**. Сложность: средняя.

**2.3 [MED] Interner: клон всего reverse-вектора на каждый новый ключ**
`D:\dev\rust\shamir-db\crates\shamir-types\src\core\interner\interner.rs:129-141` (и `:308-332` в `touch_with_id`): CAS-loop `let mut new_rev = (*cur).clone()` — O(N slots) копия вектора на **каждую** первую встречу нового имени поля. Op B уже свёл стоимость слота к refcount-bump, но сам Vec копируется целиком: холодный старт/bulk-load со schema-rich данными (10k+ уникальных полей — вложенные JSON-документы) = O(N²) слот-копий, а конкурентные вставки умножают это ретраями CAS.
Как ускорить: сегментированный spine — `ArcSwap<Vec<Arc<[OnceLock<Arc<str>>; 1024]>>>`: рост добавляет чанк (O(#чанков) клон указателей), заполнение слота — запись в OnceLock без копии вектора. Читатель: два индекса вместо одного.
Выигрыш: N первых касаний **O(N²) → O(N)**; исчезают CAS-ретраи под конкуренцией. Сложность: средняя.

**2.4 [MED] IndexManager::create_index буферизует всю таблицу декодированной в RAM**
`index_manager.rs:216-234`: полный `iter_stream` собирается в `Vec<(RecordId, InnerValue)>` (полная декодировка каждой записи) до вызова `create_index_from_records`; сам build дальше делает ещё один O(N) проход. CREATE INDEX на таблице 10M строк = вся таблица в памяти в самом дорогом (материализованном) представлении.
Как ускорить: инкрементальный build батчами (уже есть стрим!) — на каждый батч строить posting-writes и сбрасывать `set_many`; плюс `build_index_key_from_record` работает по `RecordRef` — декод в `InnerValue` не нужен вовсе, достаточно zero-copy `RecordView`.
Выигрыш: память **O(таблицы) → O(batch)**; минус полная материализация InnerValue на строку. Сложность: низкая-средняя.

**2.5 [LOW-MED] CachedStore/flush: busy-wait `yield_now` до нуля pending**
`storage_cached.rs:155-159` и `:370-372`: `while pending > 0 { yield_now().await }` — спин с переключением задач; при заваленном бэкенде это горячий цикл на воркере. Заменить на `Notify`/watch-канал от фоновых задач.

---

## 3. СТРУКТУРНЫЕ / АРХИТЕКТУРНЫЕ УСКОРЕНИЯ

**3.1 [HIGH, структурное] `RecordKey = Bytes` — heap-аллокация и косвенность на каждый 16-байтовый ключ во всём конвейере**
`D:\dev\rust\shamir-db\crates\shamir-storage\src\types.rs:8`: ключ записи — `bytes::Bytes`, при том что фактический ключ данных — всегда `RecordId([u8;16])`. Каждый переход RecordId→RecordKey (`RecordKey::copy_from_slice(id.as_bytes())` — `storage_fjall.rs:102` и десятки мест в engine/tx) = heap alloc + refcount; каждое сравнение/хэширование — через указатель. Оверлейная находка 1.4 прошлого аудита (`versioned_overlay.rs`) — частный случай этой же болезни; общий источник — тип ключа Store-API.
Как ускорить (радикально): ввести `#[repr(transparent)] struct Key128(u128)` (или enum `RecordKey { Id(u128), Raw(Bytes) }` для системных ключей переменной длины) на Store/MVCC/overlay-уровне: инлайн-копия, регистровое сравнение, Fx-хэш по u128, ноль аллокаций.
Выигрыш: минус миллионы мелких alloc на любом bulk-пути; point-op структуры (TreeIndex/scc-map по ключу) быстрее на ×1.3–2 за счёт инлайн-компаратора. Сложность: **высокая** (сквозной тип), но локализуется трейтом `Store<K>`.

**3.2 [MED-HIGH, структурное] Posting-list как `BTreeSet<RecordId>` — pointer-chasing вместо плотного массива**
`index_manager.rs:626-671`, `sorted_index_manager.rs` (lookup_range → `BTreeSet`): все не-векторные индексы возвращают и пересекают `BTreeSet<RecordId>` — аллокация на узел, кэш-недружелюбный обход, пересечение O(n log m) с промахами. Постинги по построению читаются из отсортированного prefix-скана — т.е. **уже приходят упорядоченными**.
Как ускорить: `Arc<[RecordId]>` (sorted vec) как каноническое представление; пересечение — galloping/SIMD-merge двух отсортированных срезов; объединение — k-way merge. Вместе с 1.5 (Arc вместо клона) это переводит фильтры `AND по двум индексам` из «две BTreeSet + retain» в один линейный merge по плотной памяти.
Выигрыш: intersect/union **×3–10** по памяти-локальности, минус O(n) аллокаций узлов на каждый lookup. Сложность: средняя.

**3.3 [MED, структурное] Точечные операции fjall — spawn_blocking на каждый op**
`storage_fjall.rs` — каждый `get`/`set`/`remove` = отдельный `spawn_blocking` (диспатч в пул + миграция задачи, ~1–5 µs на op, плюс потеря локальности). Батч-API есть только для `get_many`/`transact`, и даже `get_many` внутри — последовательные point-get без снапшота.
Как ускорить: (а) для read-mostly — fjall-read вообще без spawn_blocking, когда данные в memtable/block-cache (fjall 3 умеет неблокирующие пути? — проверить; иначе adaptive: inline-попытка с бюджетом); (б) шардированный воркер-луп с MPSC-батчированием point-op'ов (амортизирует диспатч на батч).
Выигрыш: point-op latency floor −1–5 µs; throughput mixed-нагрузки +10–30%. Сложность: средняя-высокая.

**3.4 [LOW-MED] TCP-фрейминг: memcpy всего ответа в scratch ради одной TLS-записи**
`D:\dev\rust\shamir-db\crates\shamir-transport-tcp\src\framing.rs:199-203` (`write_frame_into`): длина+payload склеиваются в scratch → +1 memcpy(размер ответа) на каждый ответ. Для больших SELECT-ответов (мегабайты) это заметно. Альтернатива без второй TLS-записи: сериализовать ответ в буфер с зарезервированными 4 байтами под длину (`buf.extend_from_slice(&[0;4])` до msgpack-кодирования, потом патч длины) — тогда `write_all(&buf)` без копии.
Выигрыш: −1 memcpy(ответ) на каждый response. Сложность: низкая (правка формирования ответа в request_loop).

---

## Что горячо, но не измеряется (пробелы бенчей)

- **fjall-бэкенд вообще не бенчится** — `membuffer_pump` и engine-бенчи гоняют in-memory; находки 1.1/1.2/3.3 сейчас невидимы. Нужен `storage_fjall_pump` (point get/set/scan, tempdir).
- **CachedStore scan-with-limit** — жадная материализация (1.3) не видна без бенча «скан с ранним выходом на большом кэше».
- **Startup/recovery time** при большом WAL (2.1/2.2) — не измеряется ничем; нужен soak: накопить N сегментов → замерить `open`.
- **Interner bulk cold-start** (2.3) — бенч на 10k+ уникальных полей отсутствует.
- **Equality-lookup по индексу с большим постингом** (1.5/3.2) — `engine_perf` меряет мелкие таблицы; нужен вариант с |postings| ≥ 10k.

## Топ-5 самых сочных ускорений

| # | Находка | Файл | Выигрыш | Сложность |
|---|---------|------|---------|-----------|
| 1 | fjall zero-copy `Bytes` (фича `bytes`) вместо memcpy на каждое чтение | `storage_fjall.rs:163,293,353` | ×1.2–2 disk-read, −1 alloc/op | Низкая |
| 2 | CachedStore.transact: применять Set к кэшу вместо инвалидации | `storage_cached.rs:326-345` | read-after-write ×10–100 (RAM вместо диска) | Низкая |
| 3 | Posting-кэш: `Arc<BTreeSet>` вместо полного клона на hit + sorted-slice представление | `index_manager.rs:634-671` | lookup O(&#124;postings&#124;)→O(1); intersect ×3–10 | Низкая→средняя |
| 4 | `contains_key`+`insert` двойной LSM-лукап на запись | `storage_fjall.rs:114,142,307` | до ×2 point-write | Низкая |
| 5 | `RecordKey = Bytes` → `Key128(u128)` сквозной инлайн-ключ | `storage/types.rs:8` + engine/tx | −alloc на каждый ключ везде; map-ops ×1.3–2 | Высокая (структурное) |
