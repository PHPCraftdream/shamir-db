# Ревью последних 10 коммитов — качество оптимизаций

## Состав

| # | SHA | Тип | Крейт | Эффект |
|---|-----|-----|-------|-------|
| 1 | `935b621` | perf | engine | −18% commit при n_128 (parallel join_all) |
| 2 | `f4e1d15` | perf | server | −4% subscription fanout (borrow envelope) |
| 3 | `4b6a497` | docs | — | план оптимизаций |
| 4 | `0e772ab` | feat | types+wal+tx | WAL v2 scaffolding (additive, no behavior change) |
| 5 | `ddf69af` | docs | — | аудит (этот агент) |
| 6 | `3517f18` | docs | — | сводка (этот агент) |
| 7 | `cb09dfe` | feat | engine | interner delta в commit + recovery replay |
| 8 | `32e63e1` | perf | engine | −1 durable write/commit (persist removal + checkpoint) |
| 9 | `2876ec7` | feat | tx | conflicts_with для group commit |
| 10 | `aa36838` | docs | — | pause point / session state |

**6 кодовых коммитов, 4 док-коммита.**

---

## Общая оценка: 8/10

### ✅ Сильные стороны

**1. Дисциплина коммитов — образцовая.**
Каждый коммит: rationale (почему), что изменилось, bench before/after, error semantics, idempotency analysis. Лучше чем 95% production кода.

**2. Инкрементальная стратегия Stage A — блестящая.**
WAL v3 разбит на 5 подэтапов (A1→A5), каждый independently компилируемый и тестируемый. A1+A2 — pure additive (no behavior change), A3 — plumb, A5 — remove+checkpoint. Это textbook feature-staged delivery.

**3. Backward compatibility WAL v1→v2 — корректная.**
Legacy decode через отдельный struct `WalEntryV2Legacy` + конверсия. Старые WAL файлы декодируются identically. Тест `decode_legacy_v1_produces_empty_delta` подтверждает.

**4. Crash safety invariant — чётко сформулирован и реализован.**
"WAL retains every entry whose interner delta isn't yet durable." Truncation gating проверяет `max_id_in_delta <= persisted_high_water()` — корректно.

**5. Тесты — добавлены с каждым коммитом.**
touch_with_id (5 тестов), WAL round-trip (3 теста), checkpoint gating (3 теста), conflicts_with (5 тестов), recovery interner delta (1 тест). Workspace --lib 0 failed на каждом шаге.

---

### ⚠️ Замечания (от серьёзных к мелким)

#### 1. 🔴 A5 checkpoint: `commit_version % 64` — GLOBAL, не per-table
**Файл:** `commit_phases.rs:189`, `materialize.rs:~225`
```rust
if commit_version.is_multiple_of(INTERNER_CHECKPOINT_INTERVAL)
```
**Проблема:** `commit_version` — глобальный монотонный счётчик repo. Если table A получает 64 коммита подряд, а table B — ни одного, checkpoint сработает для ВСЕХ таблиц (включая B), но только потому что A переполнила счётчик. С другой стороны, если 64 коммита распределены по 10 таблицам (по ~6 each), ни одна таблица не наберёт 64 своих коммита и checkpoint может не сработать вовремя.

**Решение:** Per-table commit counter в InternerManager. Каждая таблица считает свои коммиты отдельно и триггерит свой checkpoint.

#### 2. 🟡 A5 checkpoint: fire-and-forget `tokio::spawn` без retry
**Файл:** `commit_phases.rs:196-210`
```rust
tokio::spawn(async move {
    for table_name in repo_ck.list_table_names() {
        // persist each table...
    }
});
```
**Проблема:** Если persist падает (диск full, I/O error), checkpoint теряется. WAL truncation будет отложен навсегда → WAL растёт без ограничений. Нет max retry или WAL size cap.
**Решение:** Хотя бы log error + metric counter (уже есть `log::warn!` ✅). Но нужен WAL size guard: если WAL > N записей, принудительный sync persist.

#### 3. 🟡 A5 checkpoint: persist ALL tables на каждый 64-й commit
**Файлы:** `commit_phases.rs:196`, `materialize.rs:~220`
**Проблема:** Checkpoint персистит ВСЕ таблицы (`list_table_names()` → `get_table` → `persist` для каждой). Если 100 таблиц, а коммит затронул 1 — 99 лишних persist'ов.
**Решение:** Checkpoint только таблиц с `interner_delta_max_ids` из текущего коммита (dirty set).

#### 4. 🟡 Parallelize (935b621): clone в каждом future
```rust
.map(|(table_id, base, ops)| async move {
    apply_data_batch(repo, table_id, base.clone(), ops.clone(), ...)
})
```
**Проблема:** `base.clone()` (Arc) + `ops.clone()` (Vec) на каждый future. Для малых batch (1-2 таблицы) overhead clones может съесть win от parallelism.
**Оценка:** Bench показывает −13% при n_8 — clone overhead пренебрежимо мал на реальных данных. ✅ Норм.

#### 5. 🟢 conflicts_with: HashSet alloc на каждый вызов
**Файл:** `tx_context.rs:531-537`
```rust
let smaller_set: HashSet<(u64, &RecordKey), THasher> = smaller.write_set_keys().collect();
```
**Проблема:** O(W) alloc на каждый conflict check. Для group commit с 10 followers × 1000 keys = 10 HashSet builds.
**Оценка:** Пока `#[allow(dead_code)]` — не используется. Для Stage D можно pre-build set у leader и переиспользовать. THasher (Fx) уже применён — соответствует идеологии. ✅ Приемлемо.

#### 6. 🟢 touch_with_id: CAS-loop clone всего reverse vec
**Файл:** `interner.rs:~270`
**Проблема:** Тот же O(N) clone-and-swap pattern что в оригинальном `touch_ind`. Для recovery (replay 1000 deltas) — 1000 клонов reverse vec.
**Оценка:** Recovery — не hot path. Но это та же проблема что описана в `opt_crush/shamir-types.md` #2 (boxcar::Vec решение). ✅ Приемлемо для сейчас.

#### 7. 🟢 interner_delta: `Vec<(u64, String, u64)>` — String alloc per entry
**Файл:** `wal_entry_v2.rs:154`
**Проблема:** Каждое delta entry содержит owned `String` для field name. Для tx с 100 новыми полями — 100 String allocs в WAL serialization.
**Оценка:** Один раз на commit, не на строку. Приемлемо. Альтернатива (`CompactString` / interner reference) — premature optimization на этом этапе.

---

### 🏗️ Архитектурная оценка

**Stage A (WAL v3 + persist removal) — структурно верный подход.**
Проблема (persist в commit critical path) решена через:
1. Сделать WAL self-sufficient (delta inside)
2. Remove persist из commit
3. Background checkpoint
4. Truncation gating на high-water mark

Это классический pattern (deferred durable + checkpoint) — как in-memory WAL в PostgreSQL / group commit в MySQL. **Корректность сохранена**, performance win будет измеримым когда Stage B (lock shrink) приземлится.

**Незавершённость:** Stage B (lock shrink) — NOT STARTED. Без него A5 даёт −1 write/commit, но commit_lock всё ещё серилит все коммиты. Реальный multiplication effect (×N cores) требует B+D. Это нормально — A структурно необходимо для B.

---

## Вердикт по критериям

| Критерий | Оценка | Комментарий |
|----------|--------|-------------|
| **Корректность** | 9/10 | Crash safety invariant верный. Truncation gating корректный. Recovery replay правильно ordered (delta before ops). |
| **Производительность** | 7/10 | −18% commit, −76% scan — реальные измеримые улучшения. Но без Stage B lock-shrink полный потенциал A5 не раскрыт. |
| **Тестируемость** | 9/10 | 17+ новых тестов. Workspace --lib 0 failed на каждом шаге. TDD виден. |
| **Стиль / дисциплина** | 10/10 | Best-in-class commit messages. Incremental staging. Backward compat. |
| **Технический долг** | 7/10 | Checkpoint ALL tables (вместо dirty set). Global counter вместо per-table. Dead code (conflicts_with) — но осознанно `#[allow(dead_code)]`. |
| **Общая** | **8/10** | Высококачественная инкрементальная оптимизация. Замечания — middle-priority cleanup, не блокеры. |

---

## Рекомендации (по приоритету)

1. **Per-table checkpoint counter** — заменить global `commit_version % 64` на per-table counter в InternerManager.
2. **Dirty-set checkpoint** — persist только таблиц из `interner_delta_max_ids`, не все.
3. **WAL size guard** — если WAL > N inflight entries, sync persist всех таблиц.
4. **Реализовать Stage B** (lock shrink) — без него A5 не даёт multiplication effect.
