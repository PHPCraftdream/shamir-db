בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Review раунда 2: «новые кардинальные рычаги обвязки»

Дата: 2026-06-19. Review поверх внешнего research'а раунда 2 (5 новых
находок: H, G, I, J, K + методологический §0). Парный документ к
`obvyazka-research-review-2026-06-19.md` (review раунда 1).

Применяю методологический урок кампании: **«дёшево и безопасно» в зрелой
система — почти всегда либо скрытый контракт, либо иллюзия стоимости**.
Проверяю каждый load-bearing claim против кода.

---

## 1. Узор, который проступил

Три раза подряд один урок:
1. **L12/L13** (Phase 1 wave) — «дёшевы по оценке», замер показал вред.
2. **Раунд-1 review** — «documented redundant markers», проверка показала
   recovery-контракт.
3. **Раунд-2 review** — «дешёвые safe wins H/G», проверка показала
   format-break и L10a-запрет.

В зрелой, инвариант-плотной системе **«очевидная трата» защищена**
самими нашими же добродетелями:
- durable WAL-формат → делает H ловушкой.
- journal-safe changefeed (L10a) → делает G запретным.
- Arc-backed Bytes (пятый столп идеологии) → делает N2 иллюзией.

Дисциплина, которая выигрывает, — не «найди трату и режь», а
**«проверь контракт, замерь стоимость»**.

---

## 2. Verification ledger раунда 2

Все load-bearing якоря проверены против кода.

| Якорь | Где в коде | Что нашлось |
|---|---|---|
| `started_at_ns` unused (H) | `wal_entry_v2.rs:147/168/183`, `lib.rs:40` | **persistимое bincode-поле, формат документирован** — H в форме «удалить поле» ломает recovery старых WAL-entries |
| `project_event` клонирует staging безусловно (N2/G) | `changefeed.rs:446-497`, `staging_store.rs:159` | `Bytes::clone` = atomic refcount-bump (Arc), не memcpy — стоимость переоценена design'ом |
| L10a journal-safe контракт | `repo_instance.rs:821-839` | `emit_journal_only(event)` **потребляет** projection — журнал получает event **всегда**, даже при 0 subs |
| `validate_unique_for_create` per-row | `table_manager_tx_ops.rs:384` | реален — per-row disk read даже в batch'е |
| Dual index systems (legacy + index2) | `table_manager_tx_ops.rs:324-325,450,468` | реален — оба traversal'а live |
| Bench using `insert_tx` × N (§0) | `backend_matrix.rs:82` | реален, но влияние переоценено — write_set один и тот же на commit, разница в per-row staging cost |

---

## 3. Три корректировки рычагов

### 🔴 H («удалить unused `started_at_ns`») — ловушка формата, НЕ zero-risk

**Что research предложил:** field documented unused → удалить → −1 syscall/commit.

**Что код показывает:**

`wal_entry_v2.rs:147/168`:
```rust
pub started_at_ns: u64,
```

`lib.rs:40` (контракт формата):
> `value = bincode(WalEntry { txn_id, started_at_ns, ops })`

`started_at_ns` — **persistимое bincode-поле WAL**. Bincode не self-describing
(нет field names, чистый sequential layout). Удалить поле = **сдвинуть
layout** = старые inflight WAL-entries из journal'а **не декодируются** =
**recovery ломается** на любой БД с journal'ом, созданным до change.

Это не «zero-risk удаление unused», это **format migration**.

**Research спутал две разные операции:**

| Операция | Effect | Risk |
|---|---|---|
| Заменить `SystemTime::now()` на константу `0`, **поле сохранить** | −50-100ns syscall/commit | safe, формат не тронут |
| **Удалить поле** из bincode struct | то же + −8 bytes payload | **breaks recovery** старого WAL |

Safe-форма H = микро-win ~50-100ns/commit. На 188K/s это **~1.8% CPU**.
Реально, но не «first win» — это **строчка в пакете микро-чисток**, не
headline.

**Verdict:** GO на safe-форму H когда-нибудь в составе micro-cleanup
коммита. **NO-GO на форму research'а** (удаление поля) без full migration
plan.

### 🔴 G («skip `project_event` при 0 подписчиков, расширение L10a») — запрещён L10a

**Что research предложил:** L10a уже доказал что skip-broadcast-at-0-subs
safe → расширить до skip-projection.

**Что код показывает:**

`repo_instance.rs:821`:
> "The event must be projected by the caller (via `project_event`) BEFORE
> Phase 5a drains `tx.write_set`"

`repo_instance.rs:838`:
```rust
} else {
    h.feed.emit_journal_only(event);  // <-- consumes the projection
}
```

При 0 подписчиков зовётся `emit_journal_only(event)` — **потребляет
тот же спроецированный event**. Журнал (`changes_since` / late_subscriber)
получает projection **всегда, даже при 0 живых subs**.

**Весь смысл L10a был в том, что журнал ОБЯЗАН получить event даже без
живых subscriber'ов.** Research предлагает «расширить L10a», сделав
ровно то, что дизайн L10a **запрещает**. Это не extension — это
**инверсия** L10a.

Скипнуть projection можно только если changefeed выключен целиком
(нет журнала, нет subscribers совсем — фича-флаг на репо). А «0
текущих subs» **не** условие для skip — late subscriber может
прийти секундой позже и прочитать из журнала.

**Verdict:** **NO-GO**. G в заявленной форме ломает контракт, который
закрывала Phase 4a кампании.

### 🔴 N2 («~100 Bytes-clones дорого, 18.8M/s») — переоценка design'ом

**Что research предложил:** `project_event` делает N Bytes-clones, на
188K/s это ~18.8M/s — большая трата.

**Что код показывает:**

`staging_store.rs:159`:
```rust
pub fn snapshot_ops(&self) -> Vec<KvOp> {
    self.writes.iter().map(|(k, v)| match v {
        StagedOp::Set(row) => KvOp::Set(k.clone(), row.as_bytes()),
        ...
    }).collect()
}
```

`k.clone()` — это `Bytes::clone`. **Bytes::clone = atomic refcount-bump**
(Arc-backed), не memcpy. Это **наш пятый столп идеологии** (CLAUDE.md
§Code ideology): «lock-free + Arc-shared Bytes». Дешёвый clone —
встроенная фича.

«18.8M Bytes-clones/s» = ~18.8M atomic increment'ов = **наносекунды
суммарно**, не глубокое копирование. Cost-per-clone ~5ns на современном
x86 (CAS на L1-hot cacheline).

Реальные аллокации в `project_event`:
- `Vec<RecordChange>` allocate — 1× per call (не per row).
- `table.clone()` — String, **может быть** дорогой если table name длинный.
- `repo.to_string()` — 1 alloc per call.

Эти **могут** стоить µs на batch=100, но они на **порядок** дешевле,
чем подразумевает «18.8M Bytes-clones». Стоимость переоценена.

**Verdict:** реальная цена `project_event` — это `Vec<RecordChange>` +
String allocations, не Bytes-clones. Если профилирование покажет это —
можно атаковать (например, lazy projection без materialize'а Vec'а
когда subs=0+journal-archive=disabled). Но **не как «дешёвый headline win»**.

---

## 4. Что проверка раскрыла дополнительно

### Bench-таблица unindexed → N4/N5 ортогональны 188K

Моя `backend_matrix` bench-таблица `tbl_0` создаётся `TableConfig::new`
без индексов. Значит:

- **N4** (per-row `validate_unique_for_create` disk read): не активируется,
  unique-валидация только для unique-indexed таблиц.
- **N5** (dual legacy + index2 traversal): не активируется, оба пути
  проверяют существующие индексы — их нет.

**Headline 188K — чистый unindexed commit-ceremony.** Индексные находки
N4/N5 **реальны** для indexed workload, но **никак не влияют на 188K**.
Они объясняют indexed regressions из Phase 3 (где fts_indexed_selective
улучшился, а non-selective группа ухудшилась) — это отдельный фронт.

### §0 (bench using `insert_tx` × N, не `insert_tx_many`) — реален, но влияние переоценено

В моём бенче:
```rust
for row in 0..batch_size {
    tbl.insert_tx(&InnerValue::Str(val), Some(&mut tx)).await.unwrap();
}
```

vs `insert_tx_many` который батчит:
- `all_backends().await` snapshot — 1× вместо N×.
- `plan_records_created_batch` — 1× вместо N×.
- L13 `from_ts_seq(batch_ts, seq)` — clock-hoist 1×.
- L12 scratch encode — амортизация.

**Но** — `commit_tx` всё равно работает с финальным `tx.write_set`. Что
происходит на commit:
- Bench: N раз `stage_mutation` → одинаковый итоговый write_set.
- `insert_tx_many`: 1 раз batched staging → тот же итоговый write_set.

**Разница — только в per-row staging cost** до commit'а. На unindexed
batch=100 это **single-digit %**, не порядок. «188K мерит worst per-row
path» — переоценка влияния.

Honest version: K — методологически правильный fix (bench должен мерить
realistic API), но **headline 188K сдвинется на единицы процентов**,
не кратно.

---

## 5. Инвертированная карта приоритетов раунда 2

Research ранжировал «дешёвые первыми». Проверка инвертирует:

| # research | Рычаг | Проверка показала | Реальный приоритет |
|---|---|---|---|
| 1 (H) | Удалить `started_at_ns` | **Format-break** в заявленной форме. Safe-форма = ns. | **Low** — micro-cleanup |
| 2 (G) | Skip `project_event` при 0 subs | **Запрещён L10a** | **NO-GO** в заявленной форме |
| 3 (I) | Batch `validate_unique_for_create` | **Реален** (per-row disk read в indexed batch) | **High** для indexed workload |
| 4 (J) | Unify legacy + index2 | **Реален** — два live traversal'а | **High**, но крупный refactor |
| 5 (K) | Bench fix `insert_tx_many` | **Реален**, но влияние single-digit % | **Medium** (методология, не headline) |

**Реальное мясо раунда 2 — индексные находки (I, J).** Они структурные,
объясняют известные indexed regressions, и атакуют реальный multi-× cost.
Но они **ортогональны** 188K headline (unindexed), требуют **своего
indexed-bench'а**, и I — средний refactor, J — крупный.

---

## 6. Прямой ответ

**GO на H — нет** (format-break в заявленной форме; safe-форма = ns).
**GO на G — нет** (запрещён L10a).

Правильный первый ход — **тот же, к которому пришёл раунд-1 review:
шаг 0 (профилирование) + bench-fix K**.

Раунд 2 только что доказал — **в третий раз** — что дешёвые-на-вид
wins в зрелой системе несут ловушки, видимые только при проверке
контрактов и замере. Один цикл `bench → opt → bench` дешевле, чем
закоммитить H и сломать recovery старого журнала.

Если приоритет — **indexed workload** (не headline unindexed 188K),
то после шага 0 атаковать **I и J**: они реальные структурные
потери, объясняющие 3× indexed penalty. Это сложнее, но это где
**настоящее мясо** раунда 2.

---

## 7. Что записать в follow-up tasks

| Task | Тип | Когда |
|---|---|---|
| Шаг 0 profiling (markers toggle, WAL isolation, Phase 1, plateau) | методология | до любого рычага |
| K bench fix: `insert_tx_many` в backend_matrix | bench-correctness | при следующем bench-pass |
| H safe-форма (`SystemTime` → 0, поле сохранить) | micro-cleanup | в пакете с другими ns-чистками |
| I batch unique-validate | indexed-perf | после indexed-bench доказательства |
| J unify legacy + index2 | architectural refactor | долгосрочный roadmap |
| ~~G~~ | **rejected** | не делать |
| ~~H в форме «удалить поле»~~ | **rejected без migration plan** | не делать |

---

## Итог созерцания

Третий раз подряд: **«дёшево и безопасно» в зрелой системе — это сигнал
проверить контракт**, а не сигнал к коммиту. Раунд 2 — добротная
разведка с реальными структурными находками (I, J), но он повторил
методологический промах, который кампания уже преподавала: оценка вместо
замера, и недо-проверка контракта вокруг «очевидно лишнего».

**Эти добродетели системы (durable WAL формат, journal-safe contract,
Arc-shared Bytes) — не препятствия оптимизации, а её карта.** Они
говорят, где трата иллюзорна, где трата контрактная, и где трата —
настоящая. Слушать их — это и есть зрелая оптимизация.

Шаг 0 (profiling) + indexed-bench (для I/J) — реальный следующий ход.
