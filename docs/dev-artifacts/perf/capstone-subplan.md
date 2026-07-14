בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# CAPSTONE — lock-free WAL append «через вычитание»

**Тип:** READ-ONLY аудит + дизайн. Ни строки прод-кода. Единственный артефакт — этот doc.

**Цель.** Открыть путь к бескомпромиссному lock-free WAL-append, убрав два
санкционированных мьютекса (`WalGroupCommit.pending: tokio::Mutex<Vec>`,
`WalSegment.file: Arc<std::Mutex<File>>`) **не ради скорости** (измерено: не
горло — см. §0), а ради **структурной честности**: чтобы ни один забор не
сторожил пустоту. Условие GO — новая структура несёт доказательство
живости/crash-safety **не слабее** снятого, И выходит **меньше**, не больше.

Гипотеза красоты: постоянный single-writer-task делает «вращающегося лидера»
(симуляцию одного писателя из многих равных) НЕНУЖНЫМ. Выбор лидера
**удаляется** (не добавляется канал ПОВЕРХ него), `pending: Mutex<Vec>`
становится MPSC-каналом, `Arc<Mutex<File>>` исчезает по праву **владения**
(writer-task владеет `File` напрямую). Дизайн обязан стать МЕНЬШЕ.

---

## §0. Измеренный факт (рамка — не оптимизация)

Бенч `crates/shamir-wal/benches/wal_append.rs`, baseline-коммит `2e3bd51`
(QUICK mode, изолированный `CARGO_TARGET_DIR`), appends/sec:

| sink           | N=1   | N=64  | поведение                                   |
|----------------|-------|-------|---------------------------------------------|
| mem            | 20.5K | 91.2K | SCALES 4.4× с concurrency                   |
| file_buffered  | 7.2K  | 80.2K | масштабируется как mem (у file-лока нет конкурента) |
| file_synced    | 323   | 10.9K | fsync ≈ 95.5% synced-окна @ N=1             |

**Вердикт бенча (verbatim из коммита):** локи НЕ являются горлом пропускной
способности. Mem-throughput растёт монотонно с concurrency (lock-bound путь
бы выполаживался/регрессировал); group-commit-коалесинг делает каждого
добавленного коммиттера дешевле. fsync доминирует durable-append (~63×
полной стоимости lock+coordination+spawn); file-мьютекс берётся только
единственным избранным лидером (нулевая контенция по построению).
Маргинальная стоимость на append (~10µs) — это `tokio::spawn` + `Notify`, не
суб-µs-мьютекс.

**Следствие для рамки:** это рефакторинг ради честности комментариев и
вычитания координации, НЕ ради latency/throughput. Любая регрессия латентности
при N=1 (частый случай) — это провал, даже если структура «красивее».

---

## §1. Перепись инвариантов (the proof being replaced)

Источник доказательства — модуль-doc `wal_group_commit.rs:22-35` («This reuses
the verified leader/follower structure from `shamir-tx`'s `group_fsync.rs`
(D1b) verbatim»). **Важно:** файл `group_fsync.rs` УДАЛЁН в коммите `e9e48c9`
(«remove dead GroupFsync — superseded by file-WAL WalGroupCommit»); исходное
verbatim-доказательство сохраняется в git как `f62fe18:crates/shamir-tx/src/
group_fsync.rs` и процитировано ниже там, где это поясняет «почему per-entry
waiters, а не generation-counter». Это значит: **doc-string в
`wal_group_commit.rs` ссылается на файл, которого больше нет в дереве** — уже
сегодня лёгкая ложь аннотации (см. §5, NO-GO-аннотации это чинят).

Пронумерованный список ВСЕГО, что нынешний group-commit гарантирует:

### L1 — Никто не застрял (no stranded committer)
**Гарантия:** поздний `push` либо виден текущим лидером, либо сам выигрывает
лидерство; ни один enqueued-entry не остаётся undrained.
**Механизм сегодня =** release лидерства происходит под ТЕМ ЖЕ `pending`-локом,
что и `push`. `wal_group_commit.rs:166-175`: лидер наблюдает `p.is_empty()` и
делает `self.flushing.store(false)` **держа лок**. Поздний `push`
(`:134-135`) сериализован против этого наблюдения — либо (a) виден текущим
лидером и дренируется в следующем окне, либо (b) строго упорядочен после
`flushing=false`-store, тогда собственный CAS пушера (`:137-141`) успешен и он
становится следующим лидером.

### L2 — Нет потерянного пробуждения (no lost wakeup)
**Гарантия:** waiter, чья запись завершена ровно в окне subscribe, не пропустит
notify.
**Механизм сегодня =** `enable()`-before-check park-петля (`:146-154`):
`notified.as_mut().enable()` армит подписку на `Notify` ДО чтения
`waiter.done`. Если `complete()` (`:80-84`) вызвал `notify_one()` между
проверкой и `await`, `enable()` уже зарегистрировал permit → `await`
возвращается немедленно.

### L3 — Циркуит-брейкер (no infinite spin on a dead segment)
**Гарантия:** на write/sync error лидер отпускает лидерство и выходит, не спиня
вечно на мёртвом сегменте; следующий append выбирает свежего лидера.
**Механизм сегодня =** `:224-227`: `if !write_ok || (needs_fsync && !sync_ok) {
self.flushing.store(false); return; }`. Все waiter'ы текущего окна уже
получили `complete(false)` (`:199-220`), так что они НЕ зависают — они
просыпаются с `ok=false` и возвращают `Err` (`:155-159`).

### D1 — Тиры durability
**Гарантия:** Buffered ack после batched `write()` (ур.2); Synced ack после
batched `fsync` (ур.3); **≤1 fsync на окно**; fsync ТОЛЬКО если в окне есть
Synced-waiter.
**Механизм сегодня =** `:187-191` одна `append_batch` (write→page cache) на
окно; `:204-215` `needs_fsync = any Synced` → ровно один `sink.sync()` иначе
ноль; `:195-203` Buffered-waiter'ы будятся сразу после write, не дожидаясь
fsync. Тест `buffered_only_window_issues_no_fsync` (`wal_group_commit_tests.rs:
138-168`) делает это детерминированным.

### D2 — Completion привязан к ФИЗИЧЕСКОЙ записи
**Гарантия:** тот же таск, что дренировал tuple `(payload,version,tier,waiter)`,
— тот, что зовёт `waiter.complete()` после достижения тира. Нет «durable но
ждёт generation, которая не наступит».
**Механизм сегодня =** `wal_group_commit.rs:30-34` + `f62fe18` doc «Why
per-entry waiters, not a generation counter»: completion привязан к ФИЗИЧЕСКОМУ
entry через `Arc<Waiter>` в кортеже, а не к предсказанному эпоху. Лидер,
взявший `mem::take` (`:174`), хранит `metas: Vec<(tier, w)>` и вызывает
`w.complete()` сам (`:199-203, 216-220`).

### C1 — Cancel-safety
**Гарантия (точная):** `append` паркуется на waiter (`:146-154`); отмена
дропает future. ЧТО безопасно: отмена ДО `push` или сразу после — чистый
no-op. ЧТО НЕ безопасно: отмена ПОСЛЕ того, как work попал в `pending` и был
взят лидером — запись физически произойдёт (б命айты durable), но отменённый
caller не увидит `Ok`. Это **сознательно** не cancel-safe на границе API:
caller-side контракт это уже фиксирует — `commit.rs:216-227` («the commit
point is a *successful* Phase 4 `wal.begin` … Treat as non-cancel-safe at the
API boundary: do not race under `tokio::select!` / `tokio::time::timeout`»).
`repo_wal_manager.rs:62-63` дублирует: «cancel-safe: yes — parks on the
group-commit waiter; cancellation drops the future» — т.е. дроп future
безопасен для caller'а (RAII освобождает), но запись, уже отданная в очередь,
завершится без слушателя (recovery идемпотентно подберёт inflight-marker).

### RF1 — dirty-since-sync для background fsync
**Гарантия:** фоновый fsync-таймер fsync'ит ТОЛЬКО если был Buffered-append с
прошлого sync (не зря будит диск на тихой системе).
**Механизм сегодня =** `dirty_since_sync: AtomicBool` (`:107`), ставится при
успешном Buffered-write (`:196-198`), снимается на любом успешном sync
(`:209-211, 254-256`); `spawn_background_fsync` (`:272-287`) читает через
`take_dirty()` (`:263-265`, swap AcqRel). Weak-ref lifecycle: таск умирает,
когда дропнут последний `Arc<WalGroupCommit>` (`:273-283`). Тесты
`background_fsync_*` (`wal_group_commit_tests.rs:199-259`).

### O1 — Порядок seq / max_committed watermark
**Гарантия:** seq монотонен; `max_committed` (F6-watermark) монотонно
folds-in `batch_max_version` (`fetch_max`), даже для пустого batch.
**Механизм сегодня =** `wal_segment.rs:94,98-99` `next_seq.fetch_add` +
`max_committed.fetch_max`; пустой batch всё равно folds (`:91-96`). Лидер
вычисляет `batch_max_version` как max по окну (`:179-184`).

### B1 — Batching-семантика
**Гарантия:** N concurrent appends → 1 `write()` + ≤1 `fsync` на окно;
payloads не клонируются (split в payloads/metas).
**Механизм сегодня =** `:176-184` split без клонирования; `wal_segment.rs:
101-119` коалесинг всех фреймов в один буфер → один `write()` syscall вместо
3N. Тест `synced_fsyncs_are_batched` (`:104-136`).

### I-инварианты сегментов (НЕ в append-пути — для полноты границы)
SegmentSet несёт I1–I7 (truncation by version, active никогда не трогается,
sealed-сегменты durable перед leave-active, claim-then-delete против гонки
truncator'ов, идемпотентный replay). Это `segment_set.rs:8-13, 187-230,
261-347`. **Эти инварианты НЕ принадлежат append-горячке** — они про
rotation/truncation. Релевантны §2 лишь в части «writer-task владеет
SegmentSet → кто теперь вызывает seal/truncate».

---

## §2. Дизайн single-writer-task (the new proof)

**Прецедент в этом же коде:** `shamir-index/src/actor.rs` — `IndexActor`:
bounded `tokio::sync::mpsc` + один spawned-task дренирует канал
последовательно + `ArcSwap`-snapshot для читателей + §B14-обоснование bounded
(«unbounded channels banned without a provably bounded producer rate … under
bulk-import the queue could grow without limit and OOM»). Это РОВНО та форма,
что предлагается для WAL. Не изобретаем — переиспользуем доказанный в репо
паттерн.

### Форма

```
WalWriter {
    file: WalSink,                       // ВЛАДЕЕТ напрямую — нет Arc<Mutex<>>
    rx: mpsc::Receiver<Pending>,         // bounded MPSC
    dirty_since_sync, fsync_count, ...   // атомики остаются (телеметрия/RF1)
}
// фасад, публичный API НЕ меняется:
WalGroupCommit { tx: mpsc::Sender<Pending>, /* атомики для probe */ }
WalGroupCommit::append(payload, version, tier) -> oneshot per append
```

Writer-loop (единственный писатель):
```
while let Some(first) = rx.recv().await {
    let mut window = vec![first];
    while let Ok(p) = rx.try_recv() { window.push(p); }   // дренаж окна без лока
    // split → payloads/metas (как сейчас)
    let write_ok = file.append_batch(payloads, max_v).await.is_ok();
    // wake Buffered; fsync iff any Synced; wake Synced  (тело lead_until_drained — verbatim)
    // НО: error НЕ требует «release leadership» — просто continue (нет лидерства)
}
```

### Маппинг инвариантов §1 → новый механизм

| Инв | Сегодня | Новый механизм | Сохранён? |
|---|---|---|---|
| **L1** | release лидерства под `pending`-локом | **УДАЛЁН.** Нет выбора лидера → некого strand'ить. `recv().await` — единая точка сериализации; всё, что попало в канал, дренируется FIFO одним таском. Тривиально. | ≥ (стал тривиальным) |
| **L2** | `enable()`-before-check park | **Заменён на `oneshot`.** Writer владеет `Sender<Result>`-концом; `tx.send(res)` ставит значение ДО того, как caller `.await` его `Receiver` — oneshot не теряет (значение буферизуется в канале). Нет subscribe-window вообще. | ≥ (oneshot не имеет lost-wakeup by construction) |
| **L3** | `flushing.store(false); return` на ошибке | **УДАЛЁН.** Ошибка обрабатывается в ОДНОМ цикле: writer шлёт `Err` всем waiter'ам окна (`oneshot.send(Err)`) и `continue` к следующему `recv()`. Нет «мёртвого лидера» — таск жив и обслуживает следующее окно. Нет спина. | ≥ (стал тривиальным; но см. §3 риск паники таска) |
| **D1** | ≤1 write + ≤1 fsync/окно; fsync iff Synced | **Без изменений** — тело окна (split, write, conditional fsync) переносится verbatim в writer-loop. «Окно» теперь = `recv()` + дренаж `try_recv()` (естественный батч), вместо `mem::take`. | = (тело идентично) |
| **D2** | лидер-дренировавший зовёт complete | **Усилен type-level.** Writer-task — ЕДИНСТВЕННЫЙ, кто и читает канал, и пишет в файл, и шлёт ack. `(payload, tier, oneshot_tx)` кортеж: тот же таск, что взял его из `rx`, шлёт в его `oneshot_tx`. Невозможно структурно разорвать (нет второго writer'а). | ≥ |
| **C1** | дроп future безопасен для caller | **=, плюс уточнение.** Дроп `append`-future дропает `oneshot::Receiver`; writer'у при `send` вернётся `Err(SendError)` — он его игнорит (запись уже сделана, recovery подберёт). Caller-контракт `commit.rs:216-227` не меняется (по-прежнему non-cancel-safe на границе). | = |
| **RF1** | `dirty_since_sync` + weak-ref bg-таск | **Без изменений** — атомики и `spawn_background_fsync` остаются как есть; они координируются с writer'ом через `sink.sync()`, который теперь зовёт ЛИБО writer-loop (Synced-окно), ЛИБО bg-таск. ⚠️ **НО:** см. §3 — bg-fsync и writer-loop теперь оба зовут `sink.sync()` на ОДНОМ `WalSink`, которым ВЛАДЕЕТ writer. Это ломает single-ownership (см. ниже). | **РИСК** (см. §3, §5) |
| **O1** | `fetch_add`/`fetch_max` атомики | **Без изменений** — внутри `WalSegment`, дёргаются из writer-loop. | = |
| **B1** | split без клона + коалес-буфер | **=** — переносится verbatim. | = |

### Выбор MPSC-примитива (с обоснованием против идеологии)

| Кандидат | За | Против | Вердикт |
|---|---|---|---|
| `tokio::sync::mpsc` (bounded) | естественно async; `recv().await` + `try_recv()`-дренаж окна; УЖЕ прецедент (`IndexActor`); backpressure = `send().await` паркует producer'а (не лок) | не «lock-free» в строгом смысле (внутри — `parking_lot`-mutex на shared-state); но это НЕ hot-path-mutex в нашем смысле — producer не спинит, async-yield'ит | **ВЫБОР.** Идеология pillar-2 (async) > pillar-1 буквально: bounded async-channel — санкционированный backpressure-примитив, не «Mutex на hot path». Прецедент в репо. |
| `crossbeam-channel` |真 lock-free MPMC | НЕ async — `recv()` блокирует поток; в async writer-task пришлось бы `spawn_blocking` на каждый recv ИЛИ busy-poll; ломает «every I/O-bound op is async fn»; новой зависимости нет в `Cargo.lock` | NO — конфликт с async-pillar |
| `scc::Queue` / `crossbeam::SegQueue` | lock-free | нет встроенного async-wait → нужен СВОЙ `Notify`-слой ПОВЕРХ → это РОВНО «второй слой координации», который §делает дизайн БОЛЬШЕ | NO — ложная чистота (см. §5) |

**Bounded vs unbounded:** **BOUNDED** (cap ~ `IndexActor::DEFAULT_CHANNEL_CAPACITY`
= 1024, либо tunable). §B14-аргумент применяется дословно: WAL-append зовётся
per-commit; под bulk-import unbounded-канал рос бы без границы и OOM'ил сервер.
Bounded → producer (commit-таск) паркуется на `send().await` при заполнении —
**это и есть backpressure через async-yield, не лок**. Цена: при заполнении
commit-латентность растёт — но это корректное обратное давление, а не
deadlock (writer гарантированно дренирует, см. §3 lifecycle).

### Single-ownership `File` — теперь TYPE-LEVEL

Сегодня `WalSegment.file: Arc<Mutex<File>>` (`wal_segment.rs:40`), и комментарий
`:33-36` оправдывает мьютекс как «standalone safety» + runtime-assert
(`.expect("poisoned")`). В новом дизайне writer-task **владеет** `WalSink`
(перемещён в таск при spawn). Никакой `Arc`, никакого `Mutex` — компилятор
ГАРАНТИРУЕТ единственного писателя (move-семантика), это перестаёт быть
runtime-assert'ом и становится фактом системы типов. **Это и есть «вычитание»:**
`Arc<Mutex<File>>` → `File` (owned). ⚠️ ОДНАКО `spawn_blocking` внутри
`append_batch`/`sync` (`wal_segment.rs:101,145`) требует `'static`-замыкание —
`File` нельзя одолжить через границу `spawn_blocking` по `&`. Варианты:
(a) writer-task сам владеет `File` и НЕ использует `spawn_blocking`, а делает
синхронный `write_all` прямо в async-теле (плохо — блокирует executor-поток на
fsync ~ms); (b) writer-task держит `File` в `Option<File>`, на каждую операцию
`take()` → move в `spawn_blocking` → возврат через `oneshot`/join → `put back`
(работает, но это де-факто re-introduces ownership-ping-pong — НЕ мьютекс, но
сложность); (c) оставить `Arc<File>` БЕЗ `Mutex` (т.к. один писатель) и
`spawn_blocking` берёт clone `Arc<File>` — но `File::write_all` требует `&mut`
ИЛИ `&File` (на Unix `Write for &File` есть; на Windows — тоже, через
`std::os::windows`). **Вариант (c) — наиболее честный:** `Arc<File>` без
`Mutex`, потому что единственный писатель — type-level (только writer-task имеет
`Sender`-производитель не пишет в файл). Тогда вычитание = `Arc<Mutex<File>>` →
`Arc<File>` (исчез ИМЕННО мьютекс, `Arc` остаётся ради `spawn_blocking 'static`).
**Это слабее идеала** («`File` напрямую») но честно: мьютекс снят, `Arc` —
техническое следствие `spawn_blocking`, не координация.

### Lifecycle (leak-free)

По образцу `Drainer::spawn` (`drainer.rs:319-353`) и `spawn_background_fsync`:
- **Спавн:** в `WalGroupCommit::new` (или новом `spawn`), writer-task получает
  `rx` + owned `WalSink`. `repo_instance.rs:540` уже строит `WalGroupCommit::new`
  под `OnceCell` — single-owner гарантирован.
- **Смерть:** когда ВСЕ `Sender`-клоны дропнуты (последний `Arc<WalGroupCommit>`
  ушёл), `rx.recv()` возвращает `None` → loop завершается → owned `WalSink`
  дропается (файл закрывается). Это ЧИЩЕ weak-ref'а: канал сам сигналит EOF.
- **In-flight при дропе репо:** entries уже в канале дренируются перед `None`
  (tokio mpsc отдаёт буферизованные before close). Caller'ы, чьи `oneshot`
  ещё не получили ack — получат `Err(RecvError)` если их `Sender`-конец
  дропнут writer'ом до send; но если запись сделана — recovery подберёт.
  ⚠️ Гонка «репо дропается, commit в полёте» — та же, что сегодня (caller
  non-cancel-safe), не хуже.
- **Сосуществование с bg-fsync + дренажом:** дренаж (`drainer.rs`) НЕ пишет в
  WAL — он `recover()`/`truncate_below()` (read + delete sealed). Эти ходят
  через `WalSink::replay`/`truncate_below`, которые writer-task НЕ
  сериализует (они на sealed-сегментах / Mem-frames под `SegmentSet.inner`).
  **Это ключевой конфликт владения:** если writer-task ВЛАДЕЕТ `WalSink`
  целиком, то `replay`/`truncate_below`/`has_truncatable` (зовутся из
  drainer-таска, НЕ из writer'а) больше не могут дёргать `WalSink` напрямую —
  им нужен либо доступ через writer (послать команду в тот же канал —
  усложнение), либо `WalSink` должен оставаться `Arc`-shared между writer'ом
  (append) и drainer'ом (truncate). **Последнее ломает «writer владеет File
  напрямую».** См. §3 и §5 — это центральная трещина.

### Backpressure

Bounded MPSC: при заполнении `append` паркуется на `send().await` (async-yield,
НЕ лок, НЕ busy-spin). Producer'ы (commit-таски) тормозятся — корректное
обратное давление. Граница роста = `capacity × sizeof(Pending)` (≈ 1024 × ~160
B = ~160 KB). Writer гарантированно дренирует (он не блокируется ни на чём,
кроме самого I/O), так что заполнение — транзиентное, не deadlock.

### SegmentSet.inner mutex / MemSink.frames mutex — В ВОЛНУ или ОСТАЁТСЯ?

**ОСТАЁТСЯ (осознанно).** Обоснование:
- `SegmentSet.inner: Mutex<Inner>` (`segment_set.rs:64`) — это **метаданные**
  (sealed-список + active-`Arc` + active-seq), НЕ append-байты. Берётся на O(1)
  clone-Arc / push-в-короткий-Vec / swap-handle при rotation, НИКОГДА не
  держится через `.await` (`:54-60` это прямо документирует). Контенция:
  append-leader (rotation, редко) ↔ truncator (drainer, редко). **Это НЕ
  append-hot-path** — append-байты пишутся в `WalSegment.file`, а `inner`
  трогается лишь на rotation (раз в `WAL_SEGMENT_MAX_BYTES` = 8 MiB) и
  truncation (раз на segment-boundary).
- **Решающий аргумент:** writer-task сериализует APPEND, но truncation идёт из
  drainer-таска (`drainer.rs:253-272`) — это ДВА разных таска, легитимно
  конкурирующих за метаданные sealed-списка. `inner`-мьютекс защищает именно
  эту (редкую) кросс-таск гонку, которую writer-task НЕ устраняет (truncation
  не проходит через append-канал). Снять его = либо протолкнуть truncation
  через append-канал (writer теперь делает и unlink — расширение
  ответственности, дизайн БОЛЬШЕ), либо заменить на lock-free-структуру
  (`scc::TreeIndex` для sealed) — отдельная волна, не часть «убрать два
  append-мьютекса».
- `MemSink.frames: Mutex<Vec>` (`wal_sink.rs:28`) — аналогично: append-leader
  пишет, drainer (`truncate_below`/`has_truncatable`/`replay`) читает/retain'ит.
  Тот же кросс-таск паттерн. Если writer-task владеет MemSink — `replay`/
  `truncate` из drainer'а ломаются (см. конфликт владения выше). **Остаётся**
  как `Arc`-shared под мьютексом ИЛИ переезжает на ту же модель, что File-sink.

**Итог §2-границы:** в «волну убрать два мьютекса» входят ТОЛЬКО
`WalGroupCommit.pending` (→ MPSC) и `WalSegment.file` (→ `Arc<File>` без
Mutex). `SegmentSet.inner` и `MemSink.frames` СОЗНАТЕЛЬНО остаются — они не
append-hot, они кросс-таск-метаданные (append↔truncate), и их снятие — отдельная
тема, расширяющая ответственность writer'а (= дизайн больше = против тезиса).

---

## §3. Cancel-safety & crash-safety стресс

### (a) `append`-caller отменён, пока work уже в канале
Запись произойдёт без слушателя (`oneshot::Sender::send` вернёт
`Err(SendError)` — writer игнорит). **Допустимо?** ДА — идентично сегодняшнему
поведению: caller-контракт `commit.rs:216-227` уже объявляет путь
non-cancel-safe на границе; отменённый commit оставляет inflight WAL-marker,
который recovery (`recover_inflight_v2`) подбирает идемпотентно (LWW). **Утечка?**
НЕТ — `Pending` дренируется и пишется, `oneshot` дропается. **Дабл?** НЕТ —
запись одна; если caller ретраит, это новый `Pending` с новым version (recovery
LWW схлопывает). **НО НОВЫЙ нюанс:** bounded-канал означает, что
`append().await` может теперь париться на `send().await` (заполнен) — отмена
ЗДЕСЬ (до того, как `Pending` принят каналом) = чистый no-op (ничего не
записано). Это ДАЖЕ ЧИЩЕ сегодняшнего (сегодня `push` под `pending`-локом
всегда успевает; отмена возможна только после push). Т.е. cancel-safety **не
ухудшается**, в одной точке (full-queue) даже улучшается.

### (b) writer-task паникует/умирает
**Это главная новая опасность.** Сегодня «лидер» — эфемерный: если коммиттер-
лидер паникует в `lead_until_drained`, его `Arc<Waiter>`-окно осиротеет, НО
следующий `append` сделает CAS и станет новым лидером (L1/L3 — система
самовосстанавливается, потому что лидерство переизбирается). **В single-writer
модели нет переизбрания:** если writer-task паникует, канал закрывается с
producer-стороны?.. нет — `Sender`'ы живы, но `Receiver` (в мёртвом таске)
дропнут → все будущие `send().await` вернут `Err(closed)`, а все pending
`oneshot`-waiter'ы получат `Err(RecvError)`. Результат: **все будущие append'ы
проваливаются с `Err`, НЕ виснут** (это лучше, чем hang — fail-fast). Но БД
теряет способность писать WAL без рестарта. **Нужен ли supervisor?** 
- Минимум: writer-loop ОБЯЗАН быть panic-free by construction — тело окна
  (`append_batch`/`sync`) уже возвращает `DbResult` (не паникует на I/O-error,
  а шлёт `Err` в waiter). Единственные паники сегодня — `.expect("mutex
  poisoned")` (`wal_segment.rs:113,146`), которые в новом дизайне **исчезают
  вместе с мьютексом** (нет мьютекса — нет poisoning). Это аргумент ЗА: убирая
  мьютекс, убираем и его panic-точки.
- Желательно: тонкий supervisor (как пере-spawn в actor-паттернах) ИЛИ
  circuit-breaker-флаг, который `append` проверяет, чтобы вернуть осмысленный
  `Err(DbError::Storage("wal writer died"))` вместо непрозрачного channel-closed.
  Это ДОБАВЛЯЕТ код → надо взвесить против тезиса вычитания (см. §5).

### (c) Взаимодействие с дренажом / truncation / recovery (контракт F6)
- **ack после write (F6):** сохранён — writer шлёт ack Buffered после
  `append_batch` (write→page cache), Synced после `sync`. Идентично сегодня.
- **truncation после durable history (I2):** drainer вызывает
  `flush_all_history()` ПЕРЕД `truncate_below` (`drainer.rs:253-260`). Writer-
  task НЕ участвует в truncation. **Конфликт владения (центральный):**
  truncation трогает `WalSink` (через `truncate_below`/`has_truncatable`) из
  drainer-таска. Если writer ВЛАДЕЕТ `WalSink` эксклюзивно — drainer не может
  его дёрнуть. Разрешения:
  1. **`WalSink` остаётся `Arc`-shared** (writer держит `Arc`, drainer держит
     `Arc`); append идёт ТОЛЬКО через writer-канал (сериализован), а
     truncate/replay идут напрямую через `Arc<WalSink>` из drainer'а — БЕЗ
     контенции с append, потому что они трогают РАЗНЫЕ сегменты (active ↔
     sealed; `segment_set.rs:10-13` «append path и truncation path никогда не
     трогают один файл»). Тогда `WalSegment.file`-мьютекс снимается (один
     писатель — writer-task), но `SegmentSet.inner`-мьютекс ОСТАЁТСЯ (защищает
     sealed-список между rotation-writer'ом и truncate-drainer'ом). **Это
     рабочая модель — и она ровно та, что §2 предлагает.**
  2. Протолкнуть truncate через append-канал (writer делает unlink) — writer
     теперь и appender, и truncator → ответственность РАСТЁТ, дизайн БОЛЬШЕ.
     Отвергнуто.
- **recovery (`recover`/`replay`):** идёт через `Arc<WalSink>` из open-пути /
  drainer'а, read-only, на закрытом наборе (или Mem-frames). Не конфликтует с
  writer'ом (append append'ит active; replay читает sealed+active). Сегодня
  это под `SegmentSet.inner`-снапшотом (`segment_set.rs:235-252`) — остаётся.

**Вывод §3:** crash-контракт F6 СОХРАНЯЕТСЯ при модели §3(c).1 (WalSink
Arc-shared, append через канал, truncate/replay напрямую но на disjoint
сегментах). Главные риски: **(1) writer-task panic** (mitigated тем, что
panic-точки = мьютекс-poisoning, которые исчезают; но желателен тонкий
fail-fast); **(2) конфликт владения WalSink** вынуждает оставить `Arc<WalSink>`
+ `SegmentSet.inner`-мьютекс — т.е. «writer владеет File напрямую» достигается
лишь ЧАСТИЧНО (снят `WalSegment.file`-Mutex, но `SegmentSet.inner` остаётся).

---

## §4. Декомпозиция волны (если GO)

Кампания additive-scaffold → cutover → tests, каждый шаг gated тройным
гейтом (`fmt --check` + `clippy --all-targets -D warnings` +
`./scripts/test.sh -p shamir-wal --full`).

- **C1 — writer-task + bounded MPSC ЗА фасадом `WalGroupCommit::append`.**
  Внутренне: `WalGroupCommit { tx: mpsc::Sender<Pending>, ... }`; spawn writer-
  loop, владеющий `Arc<WalSink>`-clone'ом; `append` шлёт `(payload, ver, tier,
  oneshot_tx)` и `.await` `oneshot_rx`. **Публичный API не меняется** —
  `append/replay/truncate_below/has_truncatable/sync_now/take_dirty/
  spawn_background_fsync` те же сигнатуры → существующие тесты
  (`wal_group_commit_tests.rs`, `repo_wal_manager_tests.rs`) НЕ трогаются и
  ДОЛЖНЫ пройти как есть.
  **Gate:** весь `wal_group_commit_tests.rs` зелёный без правок; `@oracle`
  (tx+engine) зелёный (commit-путь не заметил смены).
- **C2 — удалить leader-CAS / `flushing` / `Arc<Mutex<File>>`.**
  Снять `flushing: AtomicBool` и `lead_until_drained` (поглощены writer-loop'ом).
  `WalSegment.file: Arc<Mutex<File>>` → `Arc<File>` (вариант §2(c)); убрать
  `.expect("poisoned")`-паники. `pending: Mutex<Vec>` удалён (заменён каналом в
  C1).
  **Gate:** `clippy` зелёный (нет dead `flushing`); `grep` подтверждает ноль
  `Mutex<File>` / ноль `flushing` в `shamir-wal`; bench `wal_append` собирается.
- **C3 — стресс / гонки / lifecycle + ре-бенч.**
  Новые тесты: (1) writer-task EOF при дропе последнего `Arc` (no leak — по
  образцу `background_fsync_exits_on_drop`); (2) full-queue backpressure
  (producer паркуется, дренируется, прогресс); (3) cancel mid-send (no double,
  no leak); (4) [если supervisor] writer-death → `append` возвращает `Err`, не
  виснет. Ре-бенч `wal_append` в QUICK + `BENCH_FULL=1`: **подтвердить НЕ
  регресс латентности при N=1** (частый случай — критерий §0) и не-регресс
  N=64.
  **Gate:** `./scripts/test.sh -p shamir-wal --full` зелёный; bench N=1
  mem/file_buffered/file_synced в пределах шума baseline `2e3bd51`; `@e2e`
  (db+server) зелёный (crash/recovery-тесты не сломаны).

**Точка невозврата:** C2. До C2 (только C1) — чистый additive scaffold за
фасадом, откатывается тривиально. После C2 удалены leader-механизмы.

---

## §5. Вердикт GO / NO-GO

### Решение: **УСЛОВНЫЙ GO** — с честной оговоркой о неполноте вычитания.

**Что достижимо красиво (полное вычитание, доказательство ≥):**
- `WalGroupCommit.pending: Mutex<Vec>` → bounded MPSC. L1, L2, L3 становятся
  **ТРИВИАЛЬНЫМИ** (нет выбора лидера → некого strand'ить; нет subscribe-window
  → нет lost-wakeup; ошибка в одном цикле → нет спина). D2 усиливается до
  type-level. Это **чистое вычитание**: удаляются `flushing: AtomicBool`,
  `lead_until_drained`, CAS-петля, `enable()`-before-check park. Прецедент
  доказан в репо (`IndexActor`). Доказательство НЕ слабеет — оно *исчезает за
  ненадобностью* (тривиальные инварианты не требуют доказательства).
- `WalSegment.file: Arc<Mutex<File>>` → `Arc<File>` (мьютекс снят; `Arc`
  остаётся техническим следствием `spawn_blocking 'static`). Panic-точки
  (`.expect("poisoned")`) исчезают вместе с мьютексом.

**Где вычитание НЕПОЛНО (и почему это не дисквалифицирует GO):**
- `SegmentSet.inner: Mutex<Inner>` **ОСТАЁТСЯ** — он защищает кросс-таск
  гонку append-rotation ↔ drainer-truncation, которую single-writer-task НЕ
  устраняет (truncation легитимно идёт из ДРУГОГО таска). Это **не append-hot**
  (метаданные, O(1), никогда через `.await`), и снятие его расширило бы
  ответственность writer'а (= дизайн больше = против тезиса). Идеал «writer
  владеет File напрямую» достигается частично: `WalSink` остаётся `Arc`-shared
  между writer'ом (append) и drainer'ом (truncate/replay), но это безопасно,
  т.к. они трогают disjoint-сегменты (active ↔ sealed, `segment_set.rs:10-13`).

**ОДНА главная причина GO:** два целевых append-мьютекса снимаются БЕЗ
второго слоя координации — канал ЗАМЕНЯЕТ `pending: Mutex` + `flushing: CAS` +
park-петлю (три механизма → один `recv().await`), а не добавляется поверх них.
Дизайн строго МЕНЬШЕ на append-пути, доказательство L1–L3 не слабеет (исчезает
как тривиальное), D1/D2/C1/RF1/O1/B1 сохранены ≥. Прецедент (`IndexActor`)
доказывает паттерн в этом же коде.

### Самый большой РИСК (если GO)

**Не cancel-safety (она не ухудшается, §3a) и не throughput (§0).
Главный риск — writer-task lifecycle/panic + конфликт владения WalSink.**
Конкретно: single-writer убирает само-восстановление через переизбрание
лидера — мёртвый writer-task делает БД write-incapable до рестарта (хотя
fail-fast `Err`, не hang). Mitigation: writer-loop panic-free by construction
(его единственные сегодняшние паники = мьютекс-poisoning, которые УХОДЯТ), плюс
опциональный тонкий fail-fast-флаг. Вторичный риск — конфликт владения
вынуждает `Arc<WalSink>` + сохранение `SegmentSet.inner`-мьютекса, т.е.
вычитание File-ownership лишь частичное.

### Первый ШАГ (если GO)

**C1: writer-task + bounded `tokio::sync::mpsc` строго ЗА фасадом
`WalGroupCommit::append`, публичный API байт-в-байт прежний.** Критерий
успеха — весь существующий `wal_group_commit_tests.rs` + `@oracle` проходят
БЕЗ единой правки теста. Это доказывает, что замена внутренней машинерии
наблюдаемо-эквивалентна, прежде чем удалять leader-механизмы в C2.

---

## §5-bis. NO-GO-альтернатива «красота смирения» (готовый текст аннотаций)

Если на C1/C3 выяснится, что (i) writer-death-supervisor раздувает дизайн
сверх вычитания, ИЛИ (ii) конфликт владения WalSink заставляет добавить
command-канал для truncate (второй слой координации), ИЛИ (iii) ре-бенч
показывает регресс N=1-латентности (oneshot+spawn дороже CAS+Notify) —
**откат к NO-GO** и фиксация «красоты смирения»: локи остаются, но аннотации
говорят ПОЛНУЮ правду (single-writer by construction + измеренно-не-горло +
сознательно сохранён + чинят сегодняшнюю ложь про несуществующий
`group_fsync.rs`).

**Точный текст для `WalSegment.file` (заменяет `wal_segment.rs:32-36`):**

```rust
/// Append-only, file-backed WAL segment. Splits durability:
///   - `append_batch` → `write()` to the OS page cache (level 2:
///     survives a process crash, lost only on power loss before `sync`).
///   - `sync`         → `fsync` (level 3: survives power loss).
///
/// Single-writer BY CONSTRUCTION: the sole appender is the WAL group-commit
/// coordinator's drain path (one window at a time — see
/// `wal_group_commit.rs`). The `Arc<Mutex<File>>` is therefore UNCONTENDED on
/// the hot path; it is retained, not removed, on purpose:
///   1. it is held ONLY on the blocking thread inside `spawn_blocking`
///      (`append_batch` / `sync`), NEVER across an `.await`;
///   2. the `Arc` is mandatory regardless of the lock — `spawn_blocking`
///      needs a `'static` handle, so the file must be reference-counted to
///      cross the closure boundary;
///   3. MEASURED non-bottleneck: the WAL-append contention bench
///      (`benches/wal_append.rs`, baseline `2e3bd51`) shows file-sink
///      throughput SCALING with concurrency (7.2K→80.2K appends/s, N=1→64)
///      exactly like the lockless mem sink — a lock-bound path would
///      flatten. fsync dominates a durable append ~63×; the marginal
///      ~10µs/append is `spawn_blocking` + `Notify`, not this sub-µs lock.
/// A single-writer-task rewrite (CAPSTONE, this doc) would drop the `Mutex`
/// (single ownership becomes type-level) but keep the `Arc` for
/// `spawn_blocking`; it was assessed NON-trivial only because truncation
/// (a SECOND task) shares the sink — so the lock stays, honestly annotated.
```

**Точный текст для `WalGroupCommit.pending` (заменяет `wal_group_commit.rs:99-102`):**

```rust
    // Sanctioned tokio::sync::Mutex (CLAUDE.md "Banned in hot paths"):
    // guards a tiny O(1) push / mem::take critical section, NEVER held
    // across `append_batch`/`sync` .await. Contention model: one push per
    // concurrent committer + one `mem::take` per drain window — sub-µs
    // under lock.
    //
    // SINGLE-WRITER BY CONSTRUCTION + MEASURED non-bottleneck. The rotating
    // leader (CAS on `flushing`) makes exactly one task drain at a time, so
    // this lock simulates a single writer out of many equal committers. The
    // WAL-append bench (`benches/wal_append.rs`, baseline `2e3bd51`) confirms
    // it is not the ceiling: mem-sink append SCALES 4.4× with concurrency
    // (20.5K→91.2K, N=1→64) — a lock-bound path would plateau/regress;
    // group-commit coalescing makes each added committer CHEAPER.
    //
    // NOTE: the module doc's reference to `shamir-tx`'s `group_fsync.rs`
    // (D1b) is HISTORICAL — that file was removed in `e9e48c9`; the verbatim
    // liveness proof it carried lives on in git at
    // `f62fe18:crates/shamir-tx/src/group_fsync.rs`. The L1 (no-stranded),
    // L2 (no-lost-wakeup), L3 (circuit-breaker) arguments are reproduced in
    // THIS file's module doc and in `docs/dev-artifacts/perf/capstone-subplan.md` §1.
    //
    // A single-writer-task replacement (this `Mutex` → bounded MPSC, leader
    // CAS deleted) is designed in `docs/dev-artifacts/perf/capstone-subplan.md` and is a
    // genuine subtraction; it is deferred — not rejected — pending the
    // writer-death lifecycle decision (§3b/§5).
    pending: Mutex<Vec<Pending>>,
```

(Соответственно поправить module-doc `wal_group_commit.rs:24-25`, заменив
«reuses … `group_fsync.rs` (D1b) verbatim» на ссылку на git-SHA `f62fe18` +
§1 этого дока — устранить ссылку на отсутствующий в дереве файл.)

---

## Приложение — карта файлов (file:line)

| Артефакт | Файл:строки |
|---|---|
| `WalGroupCommit` (pending Mutex, flushing CAS, append, lead_until_drained, circuit-breaker, RF1) | `crates/shamir-wal/src/wal_group_commit.rs:96-293` |
| Park-петля L2 (`enable()`-before-check) | `wal_group_commit.rs:146-154` |
| L1 release-под-локом | `wal_group_commit.rs:166-175` |
| L3 circuit-breaker | `wal_group_commit.rs:224-227` |
| `WalSegment.file: Arc<Mutex<File>>` + poisoning-panics | `crates/shamir-wal/src/wal_segment.rs:40,113,146` |
| spawn_blocking append/sync (`'static`-требование) | `wal_segment.rs:101,145` |
| `SegmentSet.inner: Mutex<Inner>` (метаданные, остаётся) | `crates/shamir-wal/src/segment_set.rs:61-65` |
| append↔truncate disjoint-сегменты | `segment_set.rs:10-13` |
| `MemSink.frames: Mutex<Vec>` (остаётся) | `crates/shamir-wal/src/wal_sink.rs:28` |
| WAL-конструкция + bg-fsync spawn + OnceCell single-owner | `crates/shamir-engine/src/repo/repo_instance.rs:497-554` |
| Drainer (truncate/recover из ДРУГОГО таска) | `crates/shamir-engine/src/tx/drainer.rs:115-275` |
| Drainer leak-free lifecycle (образец для writer-task) | `drainer.rs:319-353` |
| Caller cancel-контракт (non-cancel-safe на границе) | `crates/shamir-engine/src/tx/commit.rs:216-227` |
| `begin_grouped` (cancel-safe аннотация) | `crates/shamir-tx/src/repo_wal_manager.rs:62-93` |
| **Прецедент single-writer-actor (bounded MPSC + §B14)** | `crates/shamir-index/src/actor.rs:1-72` |
| Исходное verbatim-доказательство D1b (в git, удалено) | `f62fe18:crates/shamir-tx/src/group_fsync.rs` |
| Baseline-бенч + числа | `crates/shamir-wal/benches/wal_append.rs`; коммит `2e3bd51` |
