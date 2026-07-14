בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# V5-fix — освободить f32-граф после fit (SQ8 должен давать 4× память)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Реализуешь
> задачу #418. Бенч #412 (`docs/dev-artifacts/benchmarks/vector/2026-07-05-quantization.md`)
> вскрыл: SQ8 сейчас УВЕЛИЧИВАЕТ память (sq8 25.9 MiB > f32 15.7 MiB) — цель
> квантизации не достигнута, потому что после fit f32-граф остаётся жив
> (hnsw_rs хранит f32-векторы внутри графа), а u8-граф добавляется сверху.
> Задача: освободить f32-граф после fit, чтобы SQ8 реально экономил память.

## Корень (проверенные факты)
- `crates/shamir-index/src/vector/hnsw_adapter.rs`: поле
  `hnsw: Arc<Hnsw<'static, f32, ShamirDist>>` (стр.131) — f32-граф, живёт всегда.
- В `try_fit_and_rebuild` (стр.~738-739): `let hnsw_f32 = Arc::clone(&self.hnsw);
  let _ = hnsw_f32; // keep the f32 graph alive` — ЯВНО удерживает f32-граф.
- Пост-fit `search`/`upsert` идут ТОЛЬКО u8-путём (гейт `quantized_active()`),
  f32-граф больше НЕ читается. Компакция (`collect_live_vectors`) дуквантует
  из `vectors_u8`, НЕ из f32-графа. Снапшот v2 квант-адаптера дампит u8-граф
  (#412). Т.е. пост-fit f32-граф НЕ нужен НИКОМУ у квант-адаптера.
- Сайты доступа к `self.hnsw` (все под `Arc::clone(&self.hnsw)` / `hnsw_handle()`):
  ~стр 276 (accessor для снапшота non-quant), 524 (brute-force?), 1093/1181/
  1339/1537 (upsert/search/delete f32-путь). ВСЕ f32-путёвые сайты гейтятся
  `!quantized_active()` → пост-fit не достигаются.

## Задача
1. **Сделать f32-граф освобождаемым**: `hnsw: Arc<Hnsw<f32,ShamirDist>>` →
   `hnsw: arc_swap::ArcSwapOption<Hnsw<'static, f32, ShamirDist>>` (или
   эквивалент, дающий возможность дропнуть граф пост-fit lock-free). Все сайты
   `Arc::clone(&self.hnsw)` → `self.hnsw.load_full()` (→ `Option<Arc<...>>`).
2. **Обработка None на f32-путях**: пост-fit квант-адаптер имеет `hnsw==None`.
   Все f32-путёвые сайты уже гейтятся `!quantized_active()`, значит при
   корректной работе None там не встретится. Но код обязан быть безопасен:
   если `load_full()` даёт None на f32-пути — это инвариант-нарушение;
   верни ошибку/`unreachable!` с обоснованием ИЛИ пустой результат (обоснуй).
   НЕ паникуй в проде на нормальном пути.
3. **Дропнуть f32-граф в fit**: в `try_fit_and_rebuild` ПОСЛЕ полной публикации
   u8-графа + catch-up + drain (когда пост-fit путь уже активен и все pre-flip
   in-flight upsert'ы мигрированы) — `self.hnsw.store(None)` → Arc дропается →
   память f32-графа возвращается (когда последний in-flight pre-fit search
   отпустит свой `load_full()`-Arc — RCU, без UAF). Убери «keep alive» строки.
   Порядок: дроп ТОЛЬКО после того как гарантировано, что новые search'и идут
   u8-путём и миграция завершена (иначе pre-flip upsert в дропнутый граф).
   Обоснуй отсутствие UAF/потери в докстроке.
4. **Non-quantized адаптер НЕ трогать семантически**: `quantization==None` →
   `hnsw` всегда Some, никогда не дропается, f32-путь бит-в-бит прежний.
   Снапшот non-quant (hnsw_handle) — always Some.
5. **Перемерить бенч**: прогони `quantization_f32_vs_sq8`; sq8 RSS ДОЛЖЕН стать
   заметно МЕНЬШЕ f32 (цель — ~¼, минимум < f32). Обнови отчёт
   `docs/dev-artifacts/benchmarks/vector/2026-07-05-quantization.md` новыми цифрами +
   вывод, что 4× (или измеренная) экономия достигнута. Если QPS-разрыв
   (rescoring) остаётся — отметь как отдельный тюнинг.

## Тесты (регресс — покрыть баг)
- **memory-регресс**: тест/бенч-ассерт, что после fit RSS/footprint квант-
  адаптера НЕ больше f32 (в идеале меньше). Если точный RSS в юните хрупок —
  проверь, что `hnsw` (f32) действительно дропнут пост-fit (напр. accessor
  `f32_graph_present() -> bool` возвращает false пост-fit у квант-адаптера,
  true у non-quant / pre-fit). Это детерминированный регресс на утечку.
- back-compat: non-quant f32-путь без изменений (существующие тесты зелёные).
- concurrency: пост-fit search/upsert/delete под нагрузкой не паникуют
  (None на f32-пути не достигается) — `concurrent_upsert_across_threshold_*`
  и recall-тесты зелёные под нагрузкой.
- Не сломать снапшот/компакцию (v1 non-quant дамп через hnsw_handle работает).

## Дисциплина + гейт (ОБЯЗАТЕЛЬНО под нагрузкой — это делет конкурентный hnsw_adapter)
- Тесты ТОЛЬКО через `./scripts/test.sh` (НЕ raw cargo test).
  `./scripts/test.sh @vector @engine --full` МИНИМУМ 10 раз подряд, ВСЕ
  зелёные (пост-fit конкурентность + дроп графа — race-риск класса, что уже
  ловили в #411). `cargo clippy -p shamir-index --all-targets -- -D warnings`;
  `cargo fmt -p shamir-index -- --check`.
- Бенч: `CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench` (forward-slash!), QUICK.
- Пиллары: lock-free (ArcSwapOption, guard/Arc не через await), RCU-дроп без UAF.
  Импорты в шапке. НЕ трогать код вне задачи (только f32-граф-владение + бенч/отчёт).
- stray-логи в корне — ОТМЕТЬ, НЕ удаляй.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done
- `hnsw` (f32) освобождаемый (ArcSwapOption), дропается пост-fit у квант-
  адаптера, non-quant не тронут; f32-путёвые сайты безопасны при None.
- Бенч перемерян: sq8 RSS < f32 (цель 4×/измеренная), отчёт обновлён.
- Регресс-тест на дроп f32-графа пост-fit; back-compat + concurrency зелёные.
- `./scripts/test.sh @vector @engine --full` 10× зелёные; clippy/fmt чисты.
- Финал: как реализовано владение f32-графом (ArcSwapOption), где/почему
  безопасно дропнут (доказательство отсутствия UAF/потери под конкуренцией),
  НОВЫЕ цифры RSS f32 vs sq8 (достигнута ли экономия), вывод гейта под нагрузкой.
