# shamir-tunables — оптимизация производительности

## Обзор
Централизованные константы + RuntimeTunables (atomic reads, lock-free). ✅ Уже хорошо.

## Вывод
**Нет оптимизаций.** Все чтения — `AtomicUsize::load(Relaxed)`, lock-free, zero-overhead. Константы — compile-time.

## 🟢 Мелкие
- Добавить больше tunables в RuntimeTunables: `FULL_SCAN_BATCH`, `MAINT_SCAN_BATCH` — для runtime-тюнинга без пересборки.
- `io_frame_buffer_cap` можно `#[inline]` в вызывающем коде (уже `#[inline]` здесь ✅).
