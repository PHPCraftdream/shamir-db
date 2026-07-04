# shamir-server — оптимизация производительности

## Обзор
TCP/WS сервер: connection handling, framing, subscriptions (push, reactive), admin, tx registry.

## 🔴 Критические

### 1. Subscription filter_matches_value — работает на serde_json::Value
**Файл:** `subscriptions/filter_eval.rs:5-43`
**Сейчас:** Каждый push event конвертируется в `serde_json::Value`, затем filter прогоняется по JSON value.
**Проблема:** Double conversion: InnerValue → serde_json::Value → filter. На 50-field record × 1000 subscribers × 100 events/sec = колоссальный overhead.
**Решение:** Компилировать subscription filter в `FilterNode` (как engine делает для queries) и прогонять прямо по `InnerValue`. Избежать `inner_to_json_value` на push path.
- **Ожидаемый эффект:** −50-80% CPU на subscription matching. Устранение serde_json alloc.

### 2. make_deliver_data — сериализация на каждого subscriber
**Файл:** `subscriptions/push.rs:32-51`
**Сейчас:** `make_event_data` для каждого subscriber — payload может переиспользоваться.
**Решение:** Один раз собрать payload, затем `PushEnvelopeRef` (уже есть ✅) + borrow для каждого subscriber. Но `DeliverMode::Batch` у каждого subscriber свой — нужен cache.
- **Ожидаемый эффект:** −N× payload assembly при fanout.

---

## 🟡 Значимые

### 3. Framer — read_frame_into buffer allocation
**Проблема:** Каждый frame read может аллоцировать buffer.
**Решение:** Reusable buffer pool (BYTES buffer per connection).

### 4. decode_cache в subscriptions
**Файл:** `subscriptions/decode_cache.rs`
**Решение:** LRU cache для InnerValue → JSON конверсии. Если filter работает на InnerValue напрямую (пункт 1), cache не нужен.

---

## Приоритет
| # | Улучшение | Ожидаемый эффект | Сложность |
|---|-----------|------------------|-----------|
| 1 | FilterNode на InnerValue для subscriptions | −50-80% sub CPU | Высокая |
| 2 | Payload reuse при fanout | −N× serialize | Средняя |
| 3 | Buffer pool per connection | −alloc/frame | Низкая |
