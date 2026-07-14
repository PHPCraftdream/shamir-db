בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Review: исследование «Кардинальное ускорение обвязки»

Дата: 2026-06-19. Review поверх внешнего research'а по оптимизации
обвязки (горячий путь commit'а). Цель — не кивнуть, а проверить
load-bearing claims против кода и применить методологический урок
кампании оптимизации (`bench-snapshot-post-wave-2026-06-19.md` §2):
**оценка ≠ замер**.

---

## 1. Центральный инсайт — ✅ подтверждён, это корона

> in_memory (190K) ≈ fjall (188K) → потолок ставит обвязка, не backend.

Это самый ценный вывод исследования. **Если бы bottleneck был I/O,
in_memory был бы кратно быстрее.** Они сходятся → весь upstream до
backend-write (SSI, MVCC, WAL-сериализация, overlay-publish) — общая
стена для обоих.

**Стратегический разворот:** цель оптимизации сдвигается с backend на
обвязку.

Тонкость, которую стоит назвать явно: равенство доказывает, что
bottleneck — *общий* для обоих, т.е. весь upstream. Оно **не** доказывает,
что backend-write бесплатен — только что он не лимитирует при этом
batch/concurrency profile (w=32/b=100). На w=128 fjall падает 188K→127K,
что само по себе НЕ изолирует backend vs обвязку.

---

## 2. Verification ledger якорей исследования

Все load-bearing якоря проверены против кода:

| Якорь | Где в коде | Статус |
|---|---|---|
| `persist_markers` inline на ack-path | `materialize.rs:298` | ✅ verified |
| Комментарий «recover_inflight_v2 re-persists floor» | `materialize.rs:294-297` | ✅ дословно совпадает |
| Шаблон periodic+spawn (interner checkpoint) | `materialize.rs:318` | ✅ есть готовый паттерн с `is_multiple_of(N)` + fire-and-forget |
| `save_next_tx_id` doc: «periodically (e.g. every N commits)» | `recovery_marker.rs:41` | ✅ verified — расхождение комментарий↔код реально |
| drainer single-owner per repo | `repo_instance.rs:72` (`OnceLock`) | ✅ verified |
| WAL funnel — single coordinator | `repo_wal_manager.rs:22` (`group: Arc<WalGroupCommit>`) | ✅ verified |
| Откат single-writer-task (CAPSTONE) | `wal_group_commit.rs:44-55` | ✅ verified — откат был реальным с задокументированными причинами |

**Карта горячего пути исследования — корректна. Research сделал домашку.**

Один шум, который НЕ влияет на выводы: комментарий
`wal_group_commit.rs:65-66` «PURELY ADDITIVE: not wired ... `#[allow(dead_code)]`»
устаревший — `RepoWalManager` проводит этот `WalGroupCommit` в production
commit-путь. Старый комментарий не отменяет факт wiring.

---

## 3. Где исследование переоценивает — три поправки

### 🔴 Рычаг A «нулевой риск» — переоценён

«Documented redundant» имеет дыру. Комментарий говорит: floor
восстанавливается из **inflight WAL-entry** на recovery. Если markers
уйдут полностью в фон, появляется окно:

1. drainer дренирует версию V в `history`.
2. drainer усекает WAL-entry для V (он durable в history).
3. periodic-marker всё ещё на V' < V.
4. Crash.
5. Recovery видит: marker = V', WAL не имеет inflight для V (усечён),
   history содержит V.

На этом шаге floor ОБЯЗАН восстанавливаться из самой `history`
(max version-key), а не из marker+inflight-WAL. **Это контракт recovery,
который research не верифицировал.** До реализации A нужно прочитать
`recover_inflight_v2` и подтвердить fallback на history-scan когда
inflight отсутствует.

И аналогия с interner-checkpoint неточна:
- У interner'а hwm **гейтит truncation** (A5 gate). Periodic-persist
  означает: до следующего checkpoint'а WAL-entries не усекаются.
- У version-floor другая семантика (гейтит visibility). Periodic-persist
  означает: ack уже отдан, watermark уже advanced — marker отстаёт
  от реальности.

Это разные профили риска. A — правильный первый рычаг, но «безопасно»
требует одной верификации recovery-пути, не веры.

### 🔴 Рычаг C «новый подход» не уходит от задокументированного провала

Откат single-writer-task случился из-за **обязательного cross-task
handoff**: writer-task yield'ит → concurrent SSI-коммиттеры все валидируются
ДО публикации первого. Это сломало atomicity property, на которую engine
полагается.

Dedicated writer-thread + lock-free ring **требует того же**: коммиттер
обязан *дождаться* подтверждения, что write дошёл до tier (иначе ack
не durable). Этот wait = тот же handoff. Lock-free push в ring сам по
себе не меняет тот факт, что producer должен park'нуться до consumer'ского
ack.

Обойти можно **только** сменив контракт на `ack-before-durable` (commit
возвращается до того, как WAL physically flushed) — это семантическое
изменение durability-модели, не «оптимизация». Альтернативно — io_uring
на Linux с submit-completion — но это Linux-only и тоже требует await
completion.

C **опаснее своего ранга**, и новизна подхода сомнительна. Откат — сильный
prior. Если C делать — нужен GO/NO-GO прототип с atomicity-property как
обязательным гейтом, а не «исследование закончено».

### 🔴 Теоретический потолок (10×, 17-50×) — фантазия

Калькуляция:
> Version assign: 1 atomic (~1ns)
> WAL append: 1 write amortized (~0.1µs/row при batch/100)
> Overlay publish: 1 TreeIndex insert/row (~100-200ns)
> Cell publish: 1 hashmap insert/row (~50-100ns)
> Floor: ~0.3-0.5µs/row

**Это списывает SSI+MVCC**, а это **неустранимая цена** serializable
durable СУБД. Нельзя дойти до 0.3µs/row, сохранив serializability +
durability + reads-from-snapshot + uniqueness-checks. Postgres single
connection commits даёт ~50-200µs (с фичами SSI/MVCC), не sub-µs.

Single-commit floor research'а (256µs) **доминируется WAL
spawn_blocking round-trip + синхронизацией tokio executor'а**, а не
устранимым waste. Реальный headroom **~2-4×, не 50×**. Любая цифра
теоретического floor'а ниже ~50µs single-commit / ~3µs/row batch на
serializable durable БД — невыполнимое обещание.

---

## 4. Главный методологический провал research'а

Вся декомпозиция 248µs (20-60µs markers, 10-50µs WAL append, и т.д.) —
**оценка, не замер**.

Это **ровно тот капкан, который этот codebase нам и преподал**:
- L12 был «дёшев по оценке» — замер показал +1 alloc + memcpy per row.
- L13 был «выигрыш hoist'а» — замер показал random-order intra-batch
  ломает index locality.
- Initial post-wave snapshot заявил +27% indexed regression — diagnostic
  агент показал +6.6% было session noise, остаток diffuse.

Прежде чем строить Рычаг A на вере в «−20-60µs», надо **спрофилировать,
куда реально уходит 248µs**:
- Micro-bench с toggle `persist_markers` (test-seam on/off): single-commit
  Δ покажет реальный размер.
- Wal-append isolation bench: измерить чистый WAL append без markers.
- Phase 1 interner-remap bench: измерить `rewrite_set_bytes` отдельно.

Иначе риск: укладываем A, мерим — ноль движения, обнаруживаем что
реальная цена была в WAL-handoff (не в самих set'ах). **Measure-first
здесь не опция — это урок этого самого кода.**

И второе неизмеренное: исследование приписывает plateau w=128 (188K→127K)
«single WAL coordinator + drainer». Но не **изолировало** причину.
Альтернативы:
- tokio scheduler overhead на 128 концурентных task'ах.
- `pending` Mutex contention (tokio Mutex с 128 waiters).
- Cell-reservation scc CAS contention.
- Drainer-backpressure (`MAX_UNDRAINED_VERSIONS`).

**Тест-изолятор:** w=128 на 1 репо vs w=128 размазанный по 4 репо
(= 4 WAL coordinator'а + 4 drainer'а). Если 4-репо линейно скейлится а
1-репо плато — funnel подтверждён, B оправдан. Если 4-репо тоже
плато — bottleneck в scheduler или cell-CAS, B не поможет.

Research этого не сделал — **B строится на *предположении* о причине
plateau, не на измерении**.

---

## 5. Пересмотренная последовательность

| # | Шаг | Почему |
|---|---|---|
| **0** | **Profiling pass** | Конвертирует оценки в числа ДО любой постройки. Дёшево, безопасно. |
| 1 | A — markers в фон, но как *измеренный* win + верификация recovery-floor-from-history fallback | После шага 0 знаем реальный размер выигрыша |
| 2 | B **или** E **или** D — по результату профилирования | WAL-funnel доминирует → B. CPU (remap/SSI) доминирует → E или D. Решает измерение, не интуиция. |
| 3 | C — последним, **возможно никогда** | Откат — сильный prior. GO/NO-GO прототип с atomicity-property как обязательным гейтом. Skip если шаги 1-2 закрыли потребность. |

### Шаг 0 — Profiling pass (что конкретно нужно построить)

**Bench-suite `obvyazka_profile`:**

1. **Markers toggle bench:**
   - Cell A: full commit path (как сейчас).
   - Cell B: commit path с `persist_markers` no-op'нутым (test-seam).
   - Δ = реальная стоимость markers.

2. **WAL-append isolation:**
   - Cell A: full commit path.
   - Cell B: commit path с WAL no-op'нутым (test-seam).
   - Δ = реальная стоимость WAL-handoff.

3. **Phase 1 interner-remap isolation:**
   - Cell A: tx с tx-overlay interner (full Phase 1).
   - Cell B: tx с write-through interner (Phase 1 skip).
   - Δ = реальная стоимость remap.

4. **Plateau attribution:**
   - Cell A: w=128 на 1 repo.
   - Cell B: w=128 размазанный по 4 repo (32 writer × 4 repo).
   - Если B ≈ 4× A → funnel в WAL coordinator → B оправдан.
   - Если B ≈ A → bottleneck выше (scheduler / scc CAS) → B не помогает.

После шага 0 — пересмотреть план с фактическими числами. Это
методологическая цена за то, чтобы не повторить L12/L13.

---

## 6. Итоговая оценка research'а

**Сильные стороны (принимаю):**
- Центральный инсайт (in_memory ≈ fjall → обвязка-потолок) — железобетон.
- Карта горячего пути — корректна, якоря проверены.
- Расхождение комментарий↔код на `save_next_tx_id` — реальная находка.
- Готовый шаблон periodic+spawn рядом (interner checkpoint) — снижает
  цену Рычага A.
- Откат single-writer-task — корректный prior против C.
- Ранжирование impact/effort — разумное (за исключением занижения риска A
  и завышения новизны C).

**Слабые стороны (поправляю):**
- Теоретический floor 0.3µs/row — фантазия (списывает SSI+MVCC).
- Рычаг A «нулевой риск» — нужна верификация recovery-path.
- Рычаг C «новый подход» — не уходит от документированного провала
  без смены durability-контракта.
- Plateau attribution к WAL-funnel — *предположение*, не измерение.
- Главное: декомпозиция 248µs — **оценка**, не замер. Повторяет ошибку
  L12/L13.

**Вывод:** отличная разведка, но **унаследовала методологическую
слабость, которую кампания искореняла**. Перед любым из 6 рычагов —
шаг 0 (profiling) обязателен. После profiling'а — порядок и веса
рычагов могут измениться.

---

## 7. Прямой ответ на «A или B первым?»

**Ни то ни другое. Сначала шаг 0 (profiling).** Затем A — как калибровка
методологии (дёшево, готовый шаблон, документированно почти-безопасно,
+ одна верификация recovery). B — самый большой рычаг, но **проектировать
его до того, как замер подтвердит WAL-funnel как причину plateau, —
это повторить ошибку L12/L13**: оптимизировать по оценке, а не по числу.

Один цикл `bench → opt → bench` стоит дешевле, чем построить B и
обнаружить что bottleneck был в scheduler.
